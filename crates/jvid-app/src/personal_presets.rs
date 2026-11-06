//! Personal title, voice and effect styles. Export presets remain in `export`.
//! Each definition is an independent versioned TOML file in the platform config directory.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use egui::Ui;
use jvid_core::history::Command;
use jvid_core::model::{ClipId, Project};
use jvid_core::personal_presets::{Preset, Style};
use jvid_core::settings_transfer::{Animation, Category, PresetError, Resource};
use jvid_core::time::Time;
use serde::{Deserialize, Serialize};
use suite_ui::tokens::{color, space};
use suite_ui::widgets::{Button, Checkbox, TextInput};

use crate::i18n::tr;
use crate::keymap::Applicability;

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 1_048_576;
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// Portable installs use their own config folder too. Tests never access a real profile.
pub fn directory() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    jvid_paths::config_dir().map(|dir| dir.join("personal-presets"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    version: u32,
    pub name: String,
    pub preset: Preset,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub definition: Result<Definition, String>,
}

#[derive(Debug, Clone)]
pub struct Store {
    pub directory: PathBuf,
}

fn preset_error(error: PresetError) -> String {
    match error {
        PresetError::Transfer(error) => crate::settings_transfer::reason(error),
        _ => tr!("personal_presets.invalid").into(),
    }
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 120 && !name.chars().any(char::is_control)
}

fn io_error(error: impl std::fmt::Display) -> String {
    tr!("personal_presets.io_error", error = error.to_string())
}

fn file_id(id: &str) -> bool {
    !id.is_empty()
        && id.ends_with(".toml")
        && Path::new(id).components().count() == 1
        && !id.contains(['/', '\\'])
}

impl Store {
    pub fn entries(&self) -> Result<Vec<Entry>, String> {
        let paths = match fs::read_dir(&self.directory) {
            Ok(paths) => paths,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error(error)),
        };
        let mut entries = Vec::new();
        for path in paths {
            let path = path.map_err(io_error)?.path();
            let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !file_id(id) {
                continue;
            }
            entries.push(Entry {
                id: id.into(),
                definition: self.read(id),
            });
        }
        entries.sort_by_key(|entry| {
            entry.definition.as_ref().map_or_else(
                |_| entry.id.to_lowercase(),
                |definition| definition.name.to_lowercase(),
            )
        });
        Ok(entries)
    }

    pub fn read(&self, id: &str) -> Result<Definition, String> {
        if !file_id(id) {
            return Err(tr!("personal_presets.invalid").into());
        }
        let path = self.directory.join(id);
        if fs::symlink_metadata(&path)
            .map_err(io_error)?
            .file_type()
            .is_symlink()
        {
            return Err(tr!("personal_presets.invalid").into());
        }
        let mut text = String::new();
        fs::File::open(&path)
            .map_err(io_error)?
            .take(MAX_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(io_error)?;
        if text.len() as u64 > MAX_BYTES {
            return Err(tr!("personal_presets.invalid").into());
        }
        // Inspect version first: future definitions remain byte-for-byte untouched.
        let header: toml::Table =
            toml::from_str(&text).map_err(|_| tr!("personal_presets.invalid").to_owned())?;
        let version = header
            .get("version")
            .and_then(toml::Value::as_integer)
            .ok_or_else(|| tr!("personal_presets.invalid").to_owned())?;
        if version != i64::from(VERSION) {
            return Err(tr!("personal_presets.version", version = version));
        }
        let definition: Definition =
            toml::from_str(&text).map_err(|_| tr!("personal_presets.invalid").to_owned())?;
        if !valid_name(&definition.name) {
            return Err(tr!("personal_presets.bad_name").into());
        }
        definition
            .preset
            .validate_definition()
            .map_err(preset_error)?;
        Ok(definition)
    }

    fn unique(&self, name: &str, category: Category, except: Option<&str>) -> Result<(), String> {
        if !valid_name(name) {
            return Err(tr!("personal_presets.bad_name").into());
        }
        let duplicate = self.entries()?.iter().any(|entry| {
            Some(entry.id.as_str()) != except
                && entry.definition.as_ref().is_ok_and(|definition| {
                    definition.preset.style.category() == category
                        && definition.name.to_lowercase() == name.trim().to_lowercase()
                })
        });
        if duplicate {
            Err(tr!("personal_presets.duplicate").into())
        } else {
            Ok(())
        }
    }

    pub fn save(&self, name: &str, preset: Preset) -> Result<String, String> {
        preset.validate_definition().map_err(preset_error)?;
        self.unique(name, preset.style.category(), None)?;
        fs::create_dir_all(&self.directory).map_err(io_error)?;
        let definition = Definition {
            version: VERSION,
            name: name.trim().into(),
            preset,
        };
        let text = toml::to_string(&definition).map_err(io_error)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io_error)?
            .as_nanos();
        let id = format!(
            "p-{stamp}-{}-{}.toml",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        self.write(&id, &text, false)?;
        Ok(id)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<(), String> {
        let mut definition = self.read(id)?;
        self.unique(name, definition.preset.style.category(), Some(id))?;
        definition.name = name.trim().into();
        self.write(id, &toml::to_string(&definition).map_err(io_error)?, true)
    }

    /// Explicit deletion only. Broken/future files are never removed during a scan.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        if !file_id(id) {
            return Err(tr!("personal_presets.invalid").into());
        }
        fs::remove_file(self.directory.join(id)).map_err(io_error)
    }

    fn write(&self, id: &str, text: &str, replace: bool) -> Result<(), String> {
        if text.len() as u64 > MAX_BYTES {
            return Err(tr!("personal_presets.invalid").into());
        }
        let destination = self.directory.join(id);
        let temporary = self.directory.join(format!(
            ".{id}-{}.tmp",
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        // New files use create_new so names cannot overwrite any existing definition.
        // Replacements stage beside the original, then rename on the same filesystem.
        let writing = if replace { &temporary } else { &destination };
        let mut created = false;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(writing)?;
            created = true;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            if replace {
                fs::rename(&temporary, &destination)?;
            }
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() && created {
            let _ = fs::remove_file(writing);
        }
        result.map_err(io_error)
    }
}

/// Dependency failures disable Apply but keep the original definition available to rename,
/// delete or use after installing its font/effect. Availability is checked again on dispatch.
pub fn dependencies(
    preset: &Preset,
    fonts: &[String],
    effect: impl Fn(&str) -> bool,
) -> Vec<String> {
    match &preset.style {
        Style::Text(text)
            if !text.family.is_empty()
                && !fonts
                    .iter()
                    .any(|family| family.eq_ignore_ascii_case(&text.family)) =>
        {
            vec![if text.family.starts_with("jvid-collection-") {
                tr!("personal_presets.collected_font").into()
            } else {
                tr!("personal_presets.font", name = text.family)
            }]
        }
        Style::Effects { effects } => effects
            .iter()
            .filter(|entry| !effect(&entry.shader))
            .map(|entry| effect_advice(&entry.shader))
            .collect(),
        _ => Vec::new(),
    }
}

fn external_effect(key: &str) -> bool {
    key.contains(['/', '\\']) || key.as_bytes().get(1) == Some(&b':')
}

fn effect_advice(key: &str) -> String {
    if external_effect(key) {
        tr!("personal_presets.external_effect", path = key)
    } else {
        tr!("personal_presets.effect", name = key)
    }
}

fn load_resources(preset: &Preset) -> Result<(), String> {
    let Style::Effects { effects } = &preset.style else {
        return Ok(());
    };
    let keys: Vec<_> = effects
        .iter()
        .filter(|effect| external_effect(&effect.shader))
        .map(|effect| effect.shader.clone())
        .collect();
    crate::effects::load_exact(&keys)
        .map_err(|(key, error)| format!("{}\n{}", effect_advice(&key), error))
}

pub fn command(
    preset: &Preset,
    project: &Project,
    targets: &[ClipId],
    animation: Animation,
    fonts: &[String],
    available: impl Fn(Resource<'_>) -> bool,
) -> Result<Command, String> {
    let missing = dependencies(preset, fonts, |name| available(Resource::Effect(name)));
    if !missing.is_empty() {
        return Err(missing.join("\n"));
    }
    preset
        .settings(animation)
        .map_err(preset_error)?
        .command(project, targets, available)
        .map_err(crate::settings_transfer::reason)
}

/// Resolve collected fonts using explicit renderer provenance, never hash parsing. Only
/// readable family requirements enter storage; a destination's own alias stays local.
fn portable_font(preset: &mut Preset, sources: &[(String, String)]) -> Result<(), String> {
    if let Style::Text(text) = &mut preset.style
        && text.family.starts_with("jvid-collection-")
    {
        let original = sources
            .iter()
            .find(|(alias, original)| alias == &text.family && !original.is_empty())
            .map(|(_, original)| original.clone())
            .ok_or_else(|| tr!("personal_presets.collected_font").to_owned())?;
        text.family = original;
    }
    Ok(())
}

fn resolve_font(preset: &Preset, fonts: &mut Vec<String>, sources: &[(String, String)]) -> Preset {
    let mut resolved = preset.clone();
    if let Style::Text(text) = &mut resolved.style
        && !fonts
            .iter()
            .any(|font| font.eq_ignore_ascii_case(&text.family))
        && let Some((alias, _)) = sources
            .iter()
            .find(|(_, original)| original.eq_ignore_ascii_case(&text.family))
    {
        text.family.clone_from(alias);
        fonts.push(alias.clone());
    }
    resolved
}

fn font_sources(families: &[String]) -> Vec<(String, String)> {
    jvid_render::text::project_font_sources(families)
        .into_iter()
        .map(|source| (source.family, source.original_family))
        .collect()
}

fn capture(
    project: &Project,
    source: ClipId,
    category: Category,
    at: Time,
    animation: Animation,
) -> Result<Preset, String> {
    let mut preset =
        Preset::capture(project, source, category, at, animation).map_err(preset_error)?;
    if let Style::Text(text) = &preset.style {
        let sources = font_sources(std::slice::from_ref(&text.family));
        portable_font(&mut preset, &sources)?;
    }
    Ok(preset)
}

fn resolved_for_project(preset: &Preset, project: &Project) -> (Preset, Vec<String>) {
    if !matches!(preset.style, Style::Text(_)) {
        return (preset.clone(), Vec::new());
    }
    let families: Vec<_> = project
        .assets()
        .iter()
        .filter_map(|asset| asset.text.as_ref().map(|text| text.family.clone()))
        .collect();
    let sources = font_sources(&families);
    let mut fonts = jvid_render::text::families().to_vec();
    let resolved = resolve_font(preset, &mut fonts, &sources);
    (resolved, fonts)
}

fn prepare(
    preset: &Preset,
    project: &Project,
    targets: &[ClipId],
    animation: Animation,
) -> Result<Command, String> {
    let (resolved, fonts) = resolved_for_project(preset, project);
    crate::effects::with(|library| {
        command(
            &resolved,
            project,
            targets,
            animation,
            &fonts,
            |resource| match resource {
                Resource::Effect(name) => library.get(name).is_some_and(Result::is_ok),
                Resource::Lut(path) => Path::new(path).is_file(),
            },
        )
    })
}

pub fn applicability(
    preset: &Preset,
    project: &Project,
    targets: &[ClipId],
    animation: Animation,
) -> Applicability {
    Applicability::when(
        prepare(preset, project, targets, animation).is_ok(),
        "personal_presets.unavailable",
    )
}

fn category_label(category: Category) -> &'static str {
    match category {
        Category::Text => tr!("personal_presets.titles"),
        Category::Audio => tr!("personal_presets.voices"),
        _ => tr!("personal_presets.effects"),
    }
}

/// Useful property preview; title content is a translated sample, never saved source words.
pub fn preview(preset: &Preset) -> String {
    match &preset.style {
        Style::Text(text) => {
            let family = if text.family.is_empty() {
                tr!("inspector.sans_serif").to_owned()
            } else if text.family.starts_with("jvid-collection-") {
                tr!("inspector.font").to_owned()
            } else {
                text.family.clone()
            };
            tr!(
                "personal_presets.title_preview",
                family = family,
                size = format!("{:.0}", text.size),
                weight = text.weight
            )
        }
        Style::Audio { volume, pan, fx } => tr!(
            "personal_presets.voice_preview",
            volume = format!("{:.0}", volume * 100.0),
            pan = format!("{:.0}", pan * 100.0),
            high_pass = format!("{:.0}", fx.high_pass.freq)
        ),
        Style::Effects { effects } => {
            if effects.is_empty() {
                tr!("personal_presets.no_effects").into()
            } else {
                effects
                    .iter()
                    .map(|effect| effect.shader.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        }
    }
}

#[derive(Clone, Default)]
struct View {
    name: String,
    selected: Option<String>,
    animation: bool,
    error: Option<String>,
    entries: Vec<Entry>,
    next_scan: f64,
    delete_pending: bool,
    resource_error: Option<String>,
    texture: Option<(Preset, String, egui::TextureHandle)>,
}

/// Stable registry ID; the app's existing dispatcher owns execution and panel routing.
pub const ACTION: &str = "personal_presets";

pub fn target_category(
    ctx: &egui::Context,
    project: &Project,
    selected: &[ClipId],
) -> Option<Category> {
    let selected = selected.iter().copied().collect();
    let selection = crate::inspector::Selection::new(project, &selected, None);
    let panes = selection.panes();
    match panes.get(crate::inspector::active(ctx, &panes))? {
        crate::inspector::Pane::Text => Some(Category::Text),
        crate::inspector::Pane::Audio => Some(Category::Audio),
        _ => Some(Category::Effects),
    }
}

pub fn open_applicability(
    ctx: &egui::Context,
    project: &Project,
    selected: &[ClipId],
) -> Applicability {
    let Some(category) = target_category(ctx, project, selected) else {
        return Applicability::Disabled("settings_transfer.no_targets");
    };
    Applicability::when(
        selected.iter().all(|id| {
            jvid_core::settings_transfer::Settings::supported(project, *id)
                .is_ok_and(|categories| categories.contains(&category))
        }),
        "settings_transfer.incompatible_target",
    )
}

pub fn reveal(ctx: &egui::Context, category: Category) {
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("personal-presets-open", category as u8)),
            true,
        )
    });
    let pane = match category {
        Category::Text => crate::inspector::Pane::Text,
        Category::Audio => crate::inspector::Pane::Audio,
        _ => crate::inspector::Pane::Effects,
    };
    crate::inspector::choose(ctx, pane);
}

/// Inspector's existing pane selects a single category. View state stays outside history.
pub fn show(
    ui: &mut Ui,
    project: &Project,
    selected: &[ClipId],
    at: Time,
    category: Category,
) -> Option<Command> {
    let store = directory().map(|directory| Store { directory });
    show_from(ui, project, selected, at, category, store.as_ref())
}

fn show_from(
    ui: &mut Ui,
    project: &Project,
    selected: &[ClipId],
    at: Time,
    category: Category,
    store: Option<&Store>,
) -> Option<Command> {
    let id = ui.make_persistent_id(("personal-presets", category as u8));
    let mut view = ui
        .data_mut(|data| data.remove_temp::<View>(id))
        .unwrap_or_default();
    let mut result = None;
    let open = ui.data_mut(|data| {
        data.remove_temp::<bool>(egui::Id::new(("personal-presets-open", category as u8)))
    });
    egui::CollapsingHeader::new(category_label(category))
        .id_salt(id)
        .open(open)
        .show(ui, |ui| {
            let Some(store) = store else {
                ui.label(tr!("settings.no_config_dir"));
                return;
            };
            let now = ui.input(|input| input.time);
            if now >= view.next_scan {
                match store.entries() {
                    Ok(entries) => view.entries = entries,
                    Err(error) => view.error = Some(error),
                }
                view.next_scan = now + 1.0;
            }
            ui.ctx().request_repaint_after_secs(1.0);
            let entries: Vec<_> = view
                .entries
                .iter()
                .filter(|entry| {
                    entry.definition.as_ref().map_or(true, |definition| {
                        definition.preset.style.category() == category
                    })
                })
                .cloned()
                .collect();
            let shown = view
                .selected
                .as_deref()
                .and_then(|id| entries.iter().find(|entry| entry.id == id));
            let selected_name = shown.map_or(tr!("personal_presets.choose"), |entry| {
                entry
                    .definition
                    .as_ref()
                    .map_or(&entry.id, |definition| &definition.name)
            });
            egui::ComboBox::from_id_salt(id.with("choose"))
                .width(ui.available_width())
                .selected_text(selected_name)
                .show_ui(ui, |ui| {
                    for entry in &entries {
                        let label = entry
                            .definition
                            .as_ref()
                            .map_or(&entry.id, |definition| &definition.name);
                        if ui
                            .selectable_label(view.selected.as_ref() == Some(&entry.id), label)
                            .clicked()
                        {
                            view.selected = Some(entry.id.clone());
                            view.name = label.clone();
                            view.delete_pending = false;
                            view.error = None;
                            view.resource_error =
                                entry.definition.as_ref().ok().and_then(|definition| {
                                    load_resources(&definition.preset).err()
                                });
                        }
                    }
                });
            ui.add(
                TextInput::new(&mut view.name)
                    .hint(tr!("personal_presets.name"))
                    .width(ui.available_width()),
            );
            ui.add(Checkbox::new(
                &mut view.animation,
                tr!("settings_transfer.animation"),
            ));
            let mode = if view.animation {
                Animation::Relative
            } else {
                Animation::Static
            };
            ui.add(
                egui::Label::new(if view.animation {
                    tr!("settings_transfer.animation_hint")
                } else {
                    tr!("settings_transfer.static")
                })
                .wrap(),
            );
            let shown = view
                .selected
                .as_deref()
                .and_then(|id| entries.iter().find(|entry| entry.id == id));
            if let Some(entry) = shown {
                match &entry.definition {
                    Ok(definition) => {
                        ui.add(egui::Label::new(preview(&definition.preset)).wrap());
                        if matches!(&definition.preset.style, Style::Text(_)) {
                            let (resolved, _) = resolved_for_project(&definition.preset, project);
                            if view.texture.as_ref().is_none_or(|(preset, sample, _)| {
                                preset != &resolved || sample != tr!("personal_presets.sample")
                            }) {
                                let mut sample = match &resolved.style {
                                    Style::Text(text) => text.clone(),
                                    _ => unreachable!(),
                                };
                                sample.content = tr!("personal_presets.sample").into();
                                sample.size = (sample.size / 3.0).clamp(8.0, 64.0);
                                let image = jvid_render::text::rasterize(&sample, None, (384, 216));
                                let texture = ui.ctx().load_texture(
                                    "personal-preset-preview",
                                    egui::ColorImage::from_rgba_unmultiplied([384, 216], &image),
                                    egui::TextureOptions::LINEAR,
                                );
                                view.texture = Some((
                                    resolved,
                                    tr!("personal_presets.sample").into(),
                                    texture,
                                ));
                            }
                            if let Some((_, _, texture)) = &view.texture {
                                let width = ui.available_width().min(192.0);
                                ui.add(
                                    egui::Image::new(texture)
                                        .fit_to_exact_size(egui::vec2(width, width * 9.0 / 16.0)),
                                );
                            }
                        }
                        if let Err(reason) = prepare(&definition.preset, project, selected, mode) {
                            ui.add(
                                egui::Label::new(egui::RichText::new(reason).color(color::amber()))
                                    .wrap(),
                            );
                        }
                    }
                    Err(error) => {
                        ui.add(egui::Label::new(error).wrap());
                    }
                }
            } else if entries.is_empty() {
                ui.add(egui::Label::new(tr!("personal_presets.empty")).wrap());
            }
            ui.horizontal_wrapped(|ui| {
                let source = match selected {
                    [source] => Some(*source),
                    _ => None,
                };
                let can_save = source.is_some_and(|source| {
                    jvid_core::settings_transfer::Settings::supported(project, source)
                        .is_ok_and(|categories| categories.contains(&category))
                }) && valid_name(&view.name);
                if ui
                    .add_enabled(can_save, Button::secondary(tr!("personal_presets.save")))
                    .clicked()
                {
                    let saved = capture(project, source.unwrap(), category, at, mode)
                        .and_then(|preset| store.save(&view.name, preset));
                    match saved {
                        Ok(id) => {
                            view.selected = Some(id);
                            view.error = None;
                            view.resource_error = None;
                            view.next_scan = 0.0;
                        }
                        Err(error) => view.error = Some(error),
                    }
                }
                let definition = shown.and_then(|entry| entry.definition.as_ref().ok());
                let can_apply = view.resource_error.is_none()
                    && definition.is_some_and(|definition| {
                        applicability(&definition.preset, project, selected, mode).is_enabled()
                    });
                if ui
                    .add_enabled(can_apply, Button::secondary(tr!("personal_presets.apply")))
                    .clicked()
                {
                    // Re-read on dispatch so external edits and removed dependencies cannot use stale data.
                    let applied = store.read(&shown.unwrap().id).and_then(|definition| {
                        load_resources(&definition.preset)?;
                        prepare(&definition.preset, project, selected, mode)
                    });
                    match applied {
                        Ok(command) => {
                            result = Some(command);
                            view.error = None;
                        }
                        Err(error) => view.error = Some(error),
                    }
                }
                if ui
                    .add_enabled(
                        definition.is_some() && valid_name(&view.name),
                        Button::secondary(tr!("bin.rename")),
                    )
                    .clicked()
                {
                    view.error = store.rename(&shown.unwrap().id, &view.name).err();
                    view.next_scan = 0.0;
                }
                if ui
                    .add_enabled(shown.is_some(), Button::secondary(tr!("action.delete")))
                    .clicked()
                {
                    view.delete_pending = true;
                }
            });
            if view.delete_pending
                && let Some(entry) = shown
            {
                ui.add(egui::Label::new(tr!("personal_presets.confirm_delete")).wrap());
                ui.horizontal_wrapped(|ui| {
                    if ui.add(Button::secondary(tr!("prompt.cancel"))).clicked() {
                        view.delete_pending = false;
                    }
                    if ui.add(Button::destructive(tr!("action.delete"))).clicked() {
                        match store.delete(&entry.id) {
                            Ok(()) => {
                                view.selected = None;
                                view.name.clear();
                                view.error = None;
                                view.next_scan = 0.0;
                                view.delete_pending = false;
                            }
                            Err(error) => view.error = Some(error),
                        }
                    }
                });
            }
            if let Some(error) = &view.resource_error {
                ui.add(egui::Label::new(egui::RichText::new(error).color(color::amber())).wrap());
            }
            if let Some(error) = &view.error {
                ui.add(egui::Label::new(egui::RichText::new(error).color(color::red())).wrap());
            }
            ui.add_space(space::S4);
        });
    ui.data_mut(|data| data.insert_temp(id, view));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvid_core::history::{Created, History};
    use jvid_core::keyframe::{Easing, Keyframe, Value};
    use jvid_core::model::{Effect, NewClip, Text, TrackKind, Transform};
    use std::collections::BTreeMap;

    fn fixture(category: Category) -> (History, [ClipId; 2]) {
        let mut history = History::new(Project::default());
        let kind = if category == Category::Audio {
            TrackKind::Audio
        } else {
            TrackKind::Video
        };
        let Created::Track(track) = history
            .apply(Command::AddTrack {
                kind,
                name: "Track".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        let ids = [0, 1].map(|index| {
            let asset = if category == Category::Text {
                Command::AddText {
                    name: "Title".into(),
                    text: Text {
                        content: format!("Words {index}"),
                        size: if index == 0 { 64.0 } else { 96.0 },
                        ..Text::default()
                    },
                }
            } else {
                Command::AddSolid {
                    name: "Media".into(),
                    color: [1.0; 4],
                }
            };
            let Created::Asset(asset) = history.apply(asset).unwrap() else {
                panic!()
            };
            let Created::Clip(id) = history
                .apply(Command::AddClip {
                    track,
                    clip: NewClip {
                        asset,
                        start: Time(index * 500),
                        source_in: Time(index * 1000),
                        source_out: Time(index * 1000 + 100),
                        link: None,
                    },
                })
                .unwrap()
            else {
                panic!()
            };
            id
        });
        let mut transform = Transform {
            volume: 0.5,
            pan: -0.25,
            ..Transform::default()
        };
        if category == Category::Effects {
            transform.effects.push(Effect {
                shader: "test-effect".into(),
                params: BTreeMap::from([("strength".into(), 0.4)]),
                enabled: false,
                mask: None,
            });
        }
        let (name, value) = match category {
            Category::Text => ("text_size", 64.0),
            Category::Audio => ("volume", 0.5),
            _ => ("fx0.strength", 0.4),
        };
        let keys = BTreeMap::from([(
            name.into(),
            vec![
                Keyframe {
                    time: Time(0),
                    value: Value::Scalar(value),
                    easing: Easing::Linear,
                },
                Keyframe {
                    time: Time(99),
                    value: Value::Scalar(value * 1.5),
                    easing: Easing::Linear,
                },
            ],
        )]);
        history
            .apply(Command::SetClipProps(vec![(ids[0], transform, keys)]))
            .unwrap();
        history.mark_saved();
        (history, ids)
    }

    #[test]
    fn disk_restart_cross_project_all_categories_apply_undo_and_project_reopen() {
        for category in [Category::Text, Category::Audio, Category::Effects] {
            let directory = tempfile::tempdir().unwrap();
            let store = Store {
                directory: directory.path().join("personal-presets"),
            };
            let (source, source_ids) = fixture(category);
            let captured = Preset::capture(
                source.project(),
                source_ids[0],
                category,
                Time(50),
                Animation::Relative,
            )
            .unwrap();
            let id = store.save("Reusable", captured.clone()).unwrap();
            drop(store);
            let restarted = Store {
                directory: directory.path().join("personal-presets"),
            };
            let loaded = restarted.read(&id).unwrap();
            assert_eq!(loaded.preset, captured);
            let (mut destination, ids) = fixture(category);
            let before = destination.project().clone();
            let target = destination
                .project()
                .sequence()
                .clip(ids[1])
                .unwrap()
                .1
                .clone();
            destination
                .apply(
                    command(
                        &loaded.preset,
                        destination.project(),
                        &[ids[1]],
                        Animation::Relative,
                        &[],
                        |_| true,
                    )
                    .unwrap(),
                )
                .unwrap();
            let changed = destination.project().sequence().clip(ids[1]).unwrap().1;
            assert_eq!(
                (
                    changed.asset,
                    changed.start,
                    changed.source_in,
                    changed.source_out,
                    changed.link,
                    changed.speed,
                    changed.duration()
                ),
                (
                    target.asset,
                    target.start,
                    target.source_in,
                    target.source_out,
                    target.link,
                    target.speed,
                    target.duration()
                )
            );
            let reopened = jvid_core::file::from_json(
                &jvid_core::file::to_json(destination.project(), directory.path()).unwrap(),
                directory.path(),
                |_| false,
            )
            .unwrap();
            assert_eq!(&reopened, destination.project());
            assert_eq!(
                jvid_core::eval::frame_at(&reopened, Time(550)),
                jvid_core::eval::frame_at(destination.project(), Time(550))
            );
            if category == Category::Text {
                assert_eq!(
                    reopened
                        .asset(target.asset)
                        .unwrap()
                        .text
                        .as_ref()
                        .unwrap()
                        .content,
                    "Words 1"
                );
            }
            assert!(destination.undo());
            assert_eq!(destination.project(), &before);
            destination
                .apply(
                    command(
                        &loaded.preset,
                        destination.project(),
                        &[ids[1]],
                        Animation::Static,
                        &[],
                        |_| true,
                    )
                    .unwrap(),
                )
                .unwrap();
            assert!(
                destination
                    .project()
                    .sequence()
                    .clip(ids[1])
                    .unwrap()
                    .1
                    .keyframes
                    .is_empty()
            );
        }
    }

    #[test]
    fn rename_delete_invalid_versions_names_and_legacy_files_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().into(),
        };
        let (history, ids) = fixture(Category::Audio);
        let preset = Preset::capture(
            history.project(),
            ids[0],
            Category::Audio,
            Time(0),
            Animation::Static,
        )
        .unwrap();
        let id = store.save("Voice", preset.clone()).unwrap();
        assert!(store.save(" voice ", preset.clone()).is_err());
        assert!(store.save(" ", preset.clone()).is_err());
        assert!(store.save("bad\nname", preset.clone()).is_err());
        let oversized = Preset {
            style: Style::Effects {
                effects: vec![
                    Effect {
                        shader: "x".repeat(4096),
                        params: BTreeMap::new(),
                        enabled: true,
                        mask: None
                    };
                    300
                ],
            },
            animation_duration: None,
            keyframes: BTreeMap::new(),
        };
        assert!(store.save("Too Large", oversized).is_err());
        assert_eq!(store.entries().unwrap().len(), 1);
        store.rename(&id, "Clear Voice").unwrap();
        let again = Store {
            directory: directory.path().into(),
        };
        assert_eq!(again.read(&id).unwrap().name, "Clear Voice");
        let good = fs::read_to_string(directory.path().join(&id)).unwrap();
        let cases = [
            ("broken.toml", "[broken".into()),
            ("future.toml", "version = 99\nfuture = true\n".into()),
            ("unversioned.toml", good.replace("version = 1\n", "")),
            ("nan.toml", good.replace("volume = 0.5", "volume = nan")),
        ];
        for (name, text) in &cases {
            fs::write(directory.path().join(name), text).unwrap();
        }
        let entries = again.entries().unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.definition.is_err())
                .count(),
            4
        );
        for (name, text) in &cases {
            assert!(again.rename(name, "No Overwrite").is_err());
            assert_eq!(
                fs::read_to_string(directory.path().join(name)).unwrap(),
                *text
            );
        }
        assert!(again.read("../outside.toml").is_err());
        again.delete(&id).unwrap();
        assert!(!directory.path().join(id).exists());
        assert_eq!(again.entries().unwrap().len(), 4);
    }

    #[test]
    fn missing_fonts_effects_and_locked_batches_keep_definition_and_history() {
        for category in [Category::Text, Category::Effects] {
            let directory = tempfile::tempdir().unwrap();
            let store = Store {
                directory: directory.path().into(),
            };
            let (mut history, ids) = fixture(category);
            let mut preset = Preset::capture(
                history.project(),
                ids[0],
                category,
                Time(0),
                Animation::Static,
            )
            .unwrap();
            if let Style::Text(text) = &mut preset.style {
                text.family = "Missing Test Font".into();
            }
            let id = store.save("Offline", preset).unwrap();
            let bytes = fs::read(directory.path().join(&id)).unwrap();
            let loaded = store.read(&id).unwrap();
            let before = history.project().clone();
            let error = command(
                &loaded.preset,
                history.project(),
                &ids,
                Animation::Static,
                &[],
                |_| false,
            )
            .unwrap_err();
            assert!(error.contains(if category == Category::Text {
                "Missing Test Font"
            } else {
                "test-effect"
            }));
            assert_eq!(history.project(), &before);
            assert_eq!(fs::read(directory.path().join(&id)).unwrap(), bytes);
            let fonts = vec!["Missing Test Font".into()];
            assert!(
                command(
                    &loaded.preset,
                    history.project(),
                    &ids,
                    Animation::Static,
                    &fonts,
                    |_| true
                )
                .is_ok()
            );
            let track = history.project().sequence().clip(ids[1]).unwrap().0;
            let mut settings = history
                .project()
                .sequence()
                .track(track)
                .unwrap()
                .settings();
            settings.locked = true;
            history.apply(Command::SetTrack(settings)).unwrap();
            assert!(
                command(
                    &loaded.preset,
                    history.project(),
                    &ids,
                    Animation::Static,
                    &fonts,
                    |_| true
                )
                .is_err()
            );
        }
    }

    #[test]
    fn foreign_effect_resource_identifiers_remain_readable_and_actionable() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().into(),
        };
        for (index, key) in [
            r"C:\missing\effects\custom.wgsl",
            r"\\server\share\custom.wgsl",
            "/missing/external/custom.wgsl",
        ]
        .into_iter()
        .enumerate()
        {
            let preset = Preset {
                style: Style::Effects {
                    effects: vec![Effect {
                        shader: key.into(),
                        params: BTreeMap::new(),
                        enabled: true,
                        mask: None,
                    }],
                },
                animation_duration: None,
                keyframes: BTreeMap::new(),
            };
            let id = store
                .save(&format!("Foreign {index}"), preset.clone())
                .unwrap();
            let loaded = store.read(&id).unwrap();
            assert_eq!(loaded.preset, preset);
            assert!(dependencies(&loaded.preset, &[], |_| false)[0].contains(key));
            assert!(load_resources(&loaded.preset).unwrap_err().contains(key));
            assert_eq!(store.read(&id).unwrap().preset, preset);
        }
    }

    #[test]
    fn exact_external_effects_survive_restart_and_load_atomically_on_explicit_preflight() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("custom.wgsl");
        let second = directory.path().join("another.wgsl");
        let source = jvid_render::effect::BUNDLED[0].1;
        fs::write(&first, source).unwrap();
        fs::write(&second, "invalid shader").unwrap();
        let effects = [&first, &second]
            .map(|path| Effect {
                shader: path.to_string_lossy().into_owned(),
                params: BTreeMap::new(),
                enabled: true,
                mask: None,
            })
            .to_vec();
        let preset = Preset {
            style: Style::Effects { effects },
            animation_duration: None,
            keyframes: BTreeMap::new(),
        };
        let store = Store {
            directory: directory.path().join("presets"),
        };
        let id = store.save("External", preset.clone()).unwrap();
        let restarted = Store {
            directory: store.directory.clone(),
        };
        let loaded = restarted.read(&id).unwrap();
        assert_eq!(loaded.preset, preset);
        assert!(load_resources(&loaded.preset).is_err());
        assert!(crate::effects::with(|library| library
            .get(first.to_str().unwrap())
            .is_none()));
        fs::write(&second, source).unwrap();
        load_resources(&loaded.preset).unwrap();
        assert!(crate::effects::with(|library| library
            .get(first.to_str().unwrap())
            .is_some_and(Result::is_ok)));
        fs::remove_file(&second).unwrap();
        let error = load_resources(&loaded.preset).unwrap_err();
        assert!(error.contains(second.to_str().unwrap()));
        assert_eq!(restarted.read(&id).unwrap().preset, preset);
        assert!(crate::effects::with(|library| library
            .get(first.to_str().unwrap())
            .is_some_and(Result::is_ok)));
    }

    #[test]
    fn collected_font_metadata_becomes_portable_requirement_and_destination_alias() {
        let mut preset = Preset {
            style: Style::Text(Text {
                content: String::new(),
                family: "jvid-collection-source".into(),
                ..Text::default()
            }),
            animation_duration: None,
            keyframes: BTreeMap::new(),
        };
        let original = preset.clone();
        assert!(portable_font(&mut preset, &[]).is_err());
        assert_eq!(preset, original);
        portable_font(
            &mut preset,
            &[("jvid-collection-source".into(), "Readable Family".into())],
        )
        .unwrap();
        let text = toml::to_string(&preset).unwrap();
        assert!(!text.contains("jvid-collection-"));
        assert!(text.contains("Readable Family"));
        let mut fonts = Vec::new();
        let resolved = resolve_font(
            &preset,
            &mut fonts,
            &[(
                "jvid-collection-destination".into(),
                "Readable Family".into(),
            )],
        );
        assert!(dependencies(&resolved, &fonts, |_| true).is_empty());
        assert!(
            toml::to_string(&preset)
                .unwrap()
                .contains("Readable Family")
        );
        let mut installed = vec!["Readable Family".into()];
        assert_eq!(resolve_font(&preset, &mut installed, &[]), preset);
    }

    #[test]
    fn failed_storage_does_not_report_success_or_change_project() {
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("file");
        fs::write(&blocked, "keep").unwrap();
        let store = Store {
            directory: blocked.clone(),
        };
        let (history, ids) = fixture(Category::Audio);
        let preset = Preset::capture(
            history.project(),
            ids[0],
            Category::Audio,
            Time(0),
            Animation::Static,
        )
        .unwrap();
        assert!(store.save("Voice", preset).is_err());
        assert_eq!(fs::read_to_string(blocked).unwrap(), "keep");
        assert!(!history.is_dirty());
    }

    #[test]
    fn inspector_named_actions_work_in_narrow_panel() {
        if crate::test_util::isolate_locale() {
            return;
        }
        ui_controls(1.0, Category::Audio);
        ui_controls(1.0, Category::Text);
    }

    #[test]
    fn inspector_named_actions_work_at_high_dpi() {
        if crate::test_util::isolate_locale() {
            return;
        }
        ui_controls(1.5, Category::Audio);
        ui_controls(1.5, Category::Text);
    }

    fn ui_controls(scale: f32, category: Category) {
        use egui_kittest::kittest::Queryable;
        let directory = tempfile::tempdir().unwrap();
        let store = Store {
            directory: directory.path().into(),
        };
        let (history, ids) = fixture(category);
        let mut fresh = true;
        let previous = suite_ui::i18n::language();
        let language = std::env::var("JVID_P2_UI_LANGUAGE").ok();
        if let Some(language) = &language {
            suite_ui::i18n::set_language(language);
        }
        let artifacts = std::env::var_os("JVID_P2_UI_ARTIFACTS").map(PathBuf::from);
        let builder = egui_kittest::Harness::builder()
            .with_size([240.0, 680.0])
            .with_pixels_per_point(scale);
        let builder = if artifacts.is_some() {
            assert!(
                suite_ui::test_support::gpu(),
                "UI artifacts require a rendering backend"
            );
            builder.wgpu()
        } else {
            builder
        };
        let mut harness = builder.build_ui_state(
            |ui, history: &mut History| {
                if suite_ui::test_support::install_theme(ui, &mut fresh) {
                    return;
                }
                ui.painter().rect_filled(ui.max_rect(), 0.0, color::panel());
                if let Some(command) = show_from(
                    ui,
                    history.project(),
                    &ids[..1],
                    Time(0),
                    category,
                    Some(&store),
                ) {
                    history.apply(command).unwrap();
                }
            },
            history,
        );
        harness.run();
        harness.get_by_label(category_label(category)).click();
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .click();
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("Clear Voice");
        harness.run();
        harness.get_by_label(tr!("personal_presets.save")).click();
        harness.run();
        harness.run();
        assert_eq!(store.entries().unwrap().len(), 1);
        assert!(!harness.state().is_dirty());
        for label in [
            tr!("personal_presets.save"),
            tr!("personal_presets.apply"),
            tr!("bin.rename"),
            tr!("action.delete"),
        ] {
            assert!(
                harness.get_by_label(label).rect().right() <= 240.0,
                "{label}"
            );
        }
        harness.get_by_label(tr!("personal_presets.apply")).click();
        harness.run();
        assert!(harness.state().is_dirty());
        if let Some(artifacts) = artifacts {
            fs::create_dir_all(&artifacts).unwrap();
            harness
                .render()
                .unwrap()
                .save(artifacts.join(format!(
                    "personal-presets-{}-240-{scale}.png",
                    category as u8
                )))
                .unwrap();
        }
        harness.get_by_label(tr!("action.delete")).click();
        harness.run();
        harness.get_by_label(tr!("personal_presets.confirm_delete"));
        if language.is_some() {
            suite_ui::i18n::set_language(previous);
        }
    }
}
