//! Caption editing is a view of subtitle-track clips, independent of recognition caches.
//! Search, selection, scope and pending style controls are view state only. Row typing and
//! scrubbing use the shell's existing undo gesture route; batch edits are one command.

use std::collections::{HashMap, HashSet};

use egui::{RichText, Ui};
use j_ui::panel::Panel;
use j_ui::theme;
use j_ui::tokens::{color, space};
use j_ui::widgets::{Button, Checkbox, DragField, Dropdown, State, TextInput};
use jvid_core::captions::{self, Edit, Overrides, Scope};
use jvid_core::history::{Command, GestureKey};
use jvid_core::model::{ClipId, Project, SequenceId, Text};
use jvid_core::time::{FLICKS_PER_SECOND as S, Time};

use crate::i18n::tr;

#[derive(Default)]
pub struct Output {
    pub command: Option<Command>,
    pub gesture: Option<GestureKey>,
    pub seek: Option<Time>,
}

#[derive(Default)]
pub struct CaptionWorkspace {
    sequence: Option<SequenceId>,
    selected: HashSet<ClipId>,
    active: Option<ClipId>,
    cursors: HashMap<ClipId, usize>,
    query: String,
    replacement: String,
    scope: usize,
    reference: Option<ClipId>,
    style: Option<Text>,
    replace_overrides: bool,
}

impl CaptionWorkspace {
    pub fn show_docked(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        timeline_selection: &HashSet<ClipId>,
        playhead: Time,
        focused: bool,
    ) -> Output {
        let mut out = Output::default();
        let rows = captions::rows(project);
        if self.sequence != Some(project.sequence().id()) {
            *self = Self {
                sequence: Some(project.sequence().id()),
                scope: 2,
                ..Self::default()
            };
            self.selected = timeline_selection
                .iter()
                .copied()
                .filter(|id| rows.iter().any(|r| r.clip.id == *id))
                .collect();
        }
        self.selected
            .retain(|id| rows.iter().any(|r| r.clip.id == *id));
        self.cursors
            .retain(|id, _| rows.iter().any(|r| r.clip.id == *id));
        if !rows.iter().any(|r| Some(r.clip.id) == self.active) {
            self.active = rows.first().map(|r| r.clip.id);
        }
        if !rows.iter().any(|row| Some(row.clip.id) == self.reference) {
            self.reference = rows.first().map(|row| row.clip.id);
        }
        if self.style.is_none() {
            self.style = rows.first().map(|row| row.text.clone());
        }
        Panel::body()
            .embedded(true)
            .focused(focused)
            .show(ui, |ui| {
                if rows.is_empty() {
                    j_ui::empty_state::EmptyState::new(
                        theme::icon::LIST,
                        tr!("captions.empty"),
                        tr!("captions.empty_help"),
                    )
                    .show(ui);
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("caption_workspace")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        egui::Frame::NONE.show(ui, |ui| {
                            let width = ui.available_width();
                            ui.add(
                                TextInput::search(&mut self.query)
                                    .hint(tr!("transcript.find"))
                                    .width(width),
                            );
                            ui.add_space(space::S4);
                            ui.add(
                                TextInput::new(&mut self.replacement)
                                    .hint(tr!("captions.replacement"))
                                    .width(width),
                            );
                            ui.add_space(space::S4);
                            let scopes = [
                                tr!("export.selected"),
                                tr!("timeline.track"),
                                tr!("export.entire"),
                            ];
                            ui.add(Dropdown::new(&mut self.scope, &scopes).width(width));
                            let scope = match self.scope {
                                0 => Scope::Selection(self.selected.iter().copied().collect()),
                                1 => Scope::Track(
                                    rows.iter()
                                        .find(|r| Some(r.clip.id) == self.active)
                                        .expect("active caption")
                                        .track
                                        .id,
                                ),
                                _ => Scope::Sequence,
                            };
                            let ids = scope.ids(project);
                            let locked = rows
                                .iter()
                                .any(|r| ids.contains(&r.clip.id) && r.track.locked);
                            let ready = !ids.is_empty() && !locked;
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .add(
                                        Button::secondary(tr!("captions.replace_all"))
                                            .state(state(ready && !self.query.is_empty())),
                                    )
                                    .clicked()
                                {
                                    out.command = Some(Command::Caption(Edit::Replace {
                                        scope: scope.clone(),
                                        find: self.query.clone(),
                                        replacement: self.replacement.clone(),
                                    }));
                                }
                                if ui
                                    .add(Button::secondary(tr!("captions.merge")).state(state(
                                        self.selected.len() >= 2
                                            && !rows.iter().any(|r| {
                                                self.selected.contains(&r.clip.id) && r.track.locked
                                            }),
                                    )))
                                    .clicked()
                                {
                                    out.command = Some(Command::Caption(Edit::Merge {
                                        ids: self.selected.iter().copied().collect(),
                                    }));
                                }
                            });
                            if locked {
                                ui.label(
                                    RichText::new(tr!("timeline.track_locked"))
                                        .color(color::ink_3()),
                                );
                            }
                            egui::CollapsingHeader::new(tr!("captions.style")).show(ui, |ui| {
                                if ui
                                    .push_id("caption_reference", |ui| {
                                        j_ui::dialog::action_footer(
                                            ui,
                                            [Button::secondary(tr!("captions.use_style"))],
                                        )[0]
                                        .clicked()
                                    })
                                    .inner
                                    && let Some(row) =
                                        rows.iter().find(|r| Some(r.clip.id) == self.active)
                                {
                                    self.reference = Some(row.clip.id);
                                    self.style = Some(row.text.clone());
                                }
                                ui.checkbox(
                                    &mut self.replace_overrides,
                                    tr!("captions.replace_overrides"),
                                );
                                ui.label(
                                    RichText::new(tr!("captions.override_help"))
                                        .color(color::ink_3()),
                                );
                                if let Some(style) = self.style.as_mut() {
                                    for (id, label) in [
                                        ("font", tr!("inspector.font")),
                                        ("text_size", tr!("inspector.text_size")),
                                        ("weight", tr!("inspector.weight")),
                                        ("color", tr!("inspector.color")),
                                        ("align", tr!("inspector.align")),
                                        ("text_anchor", tr!("inspector.text_anchor")),
                                        ("outline", tr!("inspector.outline")),
                                        ("outline_color", tr!("inspector.outline_color")),
                                        ("shadow", tr!("inspector.shadow")),
                                        ("shadow_color", tr!("inspector.shadow_color")),
                                        ("box_padding", tr!("inspector.box_padding")),
                                        ("box_color", tr!("inspector.box_color")),
                                    ] {
                                        ui.push_id(id, |ui| {
                                            ui.label(label);
                                            ui.horizontal(|ui| {
                                                crate::inspector::text_fields(ui, style, id, label);
                                            });
                                        });
                                    }
                                }
                                if ui
                                    .push_id("caption_apply_style", |ui| {
                                        j_ui::dialog::action_footer(
                                            ui,
                                            [Button::secondary(tr!("captions.apply_style"))
                                                .state(state(ready))],
                                        )[0]
                                        .clicked()
                                    })
                                    .inner
                                {
                                    out.command = Some(Command::Caption(Edit::Style {
                                        scope: scope.clone(),
                                        reference: rows
                                            .iter()
                                            .find(|row| Some(row.clip.id) == self.reference)
                                            .expect("reference caption")
                                            .text
                                            .clone()
                                            .into(),
                                        style: self.style.clone().expect("style").into(),
                                        overrides: if self.replace_overrides {
                                            Overrides::Replace
                                        } else {
                                            Overrides::Keep
                                        },
                                    }));
                                }
                            });
                            ui.add_space(space::S8);
                            for (index, row) in rows.iter().enumerate().filter(|(_, r)| {
                                self.query.is_empty() || r.text.content.contains(&self.query)
                            }) {
                                ui.push_id(row.clip.id.0, |ui| {
                                    ui.separator();
                                    ui.horizontal_wrapped(|ui| {
                                        let mut picked = self.selected.contains(&row.clip.id);
                                        if ui
                                            .add(Checkbox::new(
                                                &mut picked,
                                                &(index + 1).to_string(),
                                            ))
                                            .changed()
                                        {
                                            if picked {
                                                self.selected.insert(row.clip.id);
                                            } else {
                                                self.selected.remove(&row.clip.id);
                                            }
                                            self.active = Some(row.clip.id);
                                        }
                                        ui.label(&row.track.name);
                                        if row.track.hidden || !row.clip.enabled {
                                            ui.label(
                                                RichText::new(tr!("captions.hidden"))
                                                    .color(color::ink_3()),
                                            );
                                        }
                                        if ui.add(Button::secondary(tr!("captions.seek"))).clicked()
                                        {
                                            self.active = Some(row.clip.id);
                                            out.seek = Some(row.clip.start);
                                        }
                                    });
                                    let mut text = row.text.content.clone();
                                    let response = ui.add_enabled(
                                        !row.track.locked,
                                        egui::TextEdit::multiline(&mut text)
                                            .desired_rows(2)
                                            .font(theme::content())
                                            .margin(egui::vec2(space::S8, space::S4))
                                            .hint_text(tr!("captions.text"))
                                            .desired_width(ui.available_width()),
                                    );
                                    if response.has_focus() {
                                        self.active = Some(row.clip.id);
                                        if let Some(cursor) =
                                            egui::TextEdit::load_state(ui.ctx(), response.id)
                                                .and_then(|s| s.cursor.char_range())
                                        {
                                            self.cursors
                                                .insert(row.clip.id, cursor.primary.index.into());
                                        }
                                    }
                                    if response.changed() {
                                        out.command = Some(Command::Caption(Edit::Row {
                                            id: row.clip.id,
                                            text,
                                            start: row.clip.start,
                                            end: row.clip.end(),
                                        }));
                                        out.gesture = Some(0xca70_0000_0000_0000 ^ row.clip.id.0);
                                    }
                                    let mut start = seconds(row.clip.start);
                                    let mut end = seconds(row.clip.end());
                                    ui.columns(2, |cols| {
                                        let (left, right) = cols.split_at_mut(1);
                                        for (column, label, value, key) in [
                                            (&mut left[0], tr!("captions.start"), &mut start, 1),
                                            (&mut right[0], tr!("captions.end"), &mut end, 2),
                                        ] {
                                            column.label(label);
                                            if column
                                                .add(
                                                    DragField::new(value)
                                                        .decimals(3)
                                                        .speed(0.01)
                                                        .range(0.0..=i64::MAX as f64 / S as f64)
                                                        .width(column.available_width())
                                                        .state(state(!row.track.locked)),
                                                )
                                                .changed()
                                                && let Some(value) = time(*value)
                                            {
                                                self.active = Some(row.clip.id);
                                                out.command = Some(Command::Caption(Edit::Row {
                                                    id: row.clip.id,
                                                    text: row.text.content.clone(),
                                                    start: if key == 1 {
                                                        value
                                                    } else {
                                                        row.clip.start
                                                    },
                                                    end: if key == 2 {
                                                        value
                                                    } else {
                                                        row.clip.end()
                                                    },
                                                }));
                                                out.gesture = Some(
                                                    0xca70_0000_0000_0000
                                                        ^ row.clip.id.0
                                                        ^ (key << 48),
                                                );
                                            }
                                        }
                                    });
                                    let character = self
                                        .cursors
                                        .get(&row.clip.id)
                                        .copied()
                                        .unwrap_or(row.text.content.chars().count() / 2);
                                    let can_split = !row.track.locked
                                        && row.clip.start < playhead
                                        && playhead < row.clip.end()
                                        && character > 0
                                        && character < row.text.content.chars().count();
                                    if ui
                                        .add(
                                            Button::secondary(tr!("captions.split"))
                                                .state(state(can_split)),
                                        )
                                        .on_hover_text(tr!("captions.split_help"))
                                        .clicked()
                                    {
                                        out.command = Some(Command::Caption(Edit::Split {
                                            id: row.clip.id,
                                            at: playhead,
                                            character,
                                        }));
                                        out.gesture = None;
                                    }
                                });
                            }
                        });
                    });
            });
        out
    }
}

fn state(enabled: bool) -> State {
    if enabled {
        State::Live
    } else {
        State::Disabled
    }
}
fn seconds(time: Time) -> f64 {
    time.0 as f64 / S as f64
}
fn time(seconds: f64) -> Option<Time> {
    let flicks = seconds * S as f64;
    (flicks.is_finite() && flicks >= 0.0 && flicks < i64::MAX as f64)
        .then(|| Time(flicks.round() as i64))
}

/// The shell uses the same translated refusal for row and batch edit failures.
pub(crate) fn error(error: &jvid_core::model::ModelError) -> Option<String> {
    use jvid_core::captions::Error;
    use jvid_core::model::ModelError;
    let text = match error {
        ModelError::Caption(Error::NotCaption(_)) => tr!("captions.not_caption"),
        ModelError::Caption(Error::InvalidTime) | ModelError::EmptyClip => {
            tr!("captions.invalid_time")
        }
        ModelError::Caption(Error::InvalidSplit) => tr!("captions.invalid_split"),
        ModelError::Caption(Error::InvalidMerge) => tr!("captions.invalid_merge"),
        ModelError::Caption(Error::InvalidStyle) => tr!("captions.invalid_style"),
        ModelError::Caption(Error::AnimatedTiming) => tr!("captions.animated_timing"),
        ModelError::Overlap(_) => tr!("captions.overlap"),
        _ => return None,
    };
    Some(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use jvid_core::history::History;

    fn fixture() -> History {
        let mut h = History::new(Project::default());
        let mut settings = h.project().settings().clone();
        settings.width = 320;
        settings.height = 180;
        h.apply(Command::SetSettings(settings)).unwrap();
        h.apply(Command::ImportSubtitles {
            name: "Subtitles".into(),
            cues: vec![jvid_core::srt::Cue {
                start: Time(0),
                end: Time(2 * S),
                text: "café 字幕 日本語 漢字 한글".into(),
            }],
            style: Text {
                size: 30.0,
                ..Text::default()
            },
        })
        .unwrap();
        History::new(h.project().clone())
    }

    struct Editor {
        history: History,
        panel: CaptionWorkspace,
        fresh: bool,
    }

    fn harness(width: f32, height: f32, scale: f32, gpu: bool) -> Harness<'static, Editor> {
        let builder = Harness::builder()
            .with_size([width, height])
            .with_pixels_per_point(scale);
        let builder = if gpu { builder.wgpu() } else { builder };
        builder.build_ui_state(
            |ui, editor: &mut Editor| {
                if j_ui::test_support::install_theme(ui, &mut editor.fresh) {
                    return;
                }
                if ui.ctx().input(|i| i.pointer.any_pressed()) {
                    editor.history.end_gesture();
                }
                let out = editor.panel.show_docked(
                    ui,
                    editor.history.project(),
                    &HashSet::new(),
                    Time(S),
                    true,
                );
                if let Some(command) = out.command {
                    match out.gesture {
                        Some(key) => editor.history.apply_coalesced(key, command),
                        None => editor.history.apply(command),
                    }
                    .unwrap();
                }
            },
            Editor {
                history: fixture(),
                panel: Default::default(),
                fresh: true,
            },
        )
    }

    #[test]
    fn native_row_typing_coalesces_then_split_undo_and_disk_reopen_match_srt() {
        if crate::test_util::isolate_locale() {
            return;
        }
        j_ui::i18n::set_language("en");
        let mut h = harness(240.0, 800.0, 1.0, false);
        h.run();
        let before = h.state().history.project().clone();
        h.get_all_by_role(egui::accesskit::Role::MultilineTextInput)
            .find(|node| node.value().as_deref() == Some("café 字幕 日本語 漢字 한글"))
            .unwrap()
            .focus();
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text("你好🙂 café".into()));
        h.run();
        h.event(egui::Event::Text("!".into()));
        h.run();
        assert_eq!(
            captions::rows(h.state().history.project())[0].text.content,
            "你好🙂 café!"
        );
        h.key_press(egui::Key::Home);
        h.run();
        for _ in 0..3 {
            h.key_press(egui::Key::ArrowRight);
            h.run();
        }
        h.get_by_label(tr!("captions.split")).click();
        h.run();
        let project = h.state().history.project();
        assert_eq!(captions::rows(project).len(), 2);
        assert_eq!(project.subtitle_cues()[0].text, "你好🙂");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("字幕.jvid");
        crate::project_file::save(project, &path).unwrap();
        let reopened = crate::project_file::load(&path).unwrap();
        assert_eq!(&reopened, project);
        assert_eq!(
            jvid_core::srt::write(&reopened.subtitle_cues(), (Time(0), Time(2 * S))),
            "1\n00:00:00,000 --> 00:00:01,000\n你好🙂\n\n2\n00:00:01,000 --> 00:00:02,000\n café!\n\n"
        );
        assert!(h.state_mut().history.undo());
        assert_eq!(
            captions::rows(h.state().history.project())[0].text.content,
            "你好🙂 café!"
        );
        assert!(h.state_mut().history.undo());
        assert_eq!(h.state().history.project(), &before);
        assert!(!h.state().history.can_undo());
    }

    #[test]
    fn repeated_shared_style_applies_follow_authoritative_reference_after_undo() {
        if crate::test_util::isolate_locale() {
            return;
        }
        j_ui::i18n::set_language("en");
        let mut h = harness(600.0, 1400.0, 1.0, false);
        h.run();
        let before = h.state().history.project().clone();
        h.get_by_label(tr!("captions.style")).click();
        h.run();
        for size in [60.0, 72.0] {
            h.state_mut().panel.style.as_mut().unwrap().size = size;
            h.run();
            h.get_by_label(tr!("captions.apply_style")).click();
            h.run();
            assert_eq!(
                captions::rows(h.state().history.project())[0].text.size,
                size
            );
        }
        assert!(h.state_mut().history.undo());
        h.run();
        assert_eq!(
            captions::rows(h.state().history.project())[0].text.size,
            60.0
        );
        h.state_mut().panel.style.as_mut().unwrap().size = 80.0;
        h.run();
        h.get_by_label(tr!("captions.apply_style")).click();
        h.run();
        assert_eq!(
            captions::rows(h.state().history.project())[0].text.size,
            80.0
        );
        assert!(h.state_mut().history.undo() && h.state_mut().history.undo());
        assert_eq!(h.state().history.project(), &before);
        assert!(!h.state().history.can_undo());
    }

    #[test]
    fn narrow_translated_panel_controls_stay_in_bounds_and_view_state_never_edits_project() {
        if crate::test_util::isolate_locale() {
            return;
        }
        if !j_ui::test_support::gpu() {
            return;
        }
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/caption-workspace");
        std::fs::create_dir_all(&dir).unwrap();
        for language in ["en", "de", "ja"] {
            j_ui::i18n::set_language(language);
            for scale in [1.0, 1.5] {
                let mut h = harness(240.0, 800.0, scale, true);
                h.run();
                let before = h.state().history.project().clone();
                h.ctx.fonts_mut(|fonts| {
                    let missing = fonts
                        .layout_no_wrap("\u{10FFFD}".into(), theme::content(), color::ink())
                        .rows[0]
                        .glyphs[0]
                        .uv_rect;
                    for character in "字幕日本語漢字한글".chars() {
                        let glyph = fonts
                            .layout_no_wrap(character.to_string(), theme::content(), color::ink())
                            .rows[0]
                            .glyphs[0]
                            .uv_rect;
                        assert_ne!(glyph, missing, "{language}: missing {character}");
                    }
                });
                h.render()
                    .unwrap()
                    .save(dir.join(format!("captions-rows-{language}-240-{scale}x.png")))
                    .unwrap();
                h.get_by_label(tr!("captions.style")).click();
                h.run();
                for role in [
                    egui::accesskit::Role::Button,
                    egui::accesskit::Role::TextInput,
                    egui::accesskit::Role::MultilineTextInput,
                ] {
                    for node in h.get_all_by_role(role) {
                        assert!(
                            node.rect().left() >= -0.5 && node.rect().right() <= 240.5,
                            "{language} {scale}: {:?}",
                            node.rect()
                        );
                    }
                }
                h.render()
                    .unwrap()
                    .save(dir.join(format!("captions-{language}-240-{scale}x.png")))
                    .unwrap();
                h.state_mut().panel.query = "missing".into();
                h.state_mut().panel.replacement = "replacement".into();
                h.state_mut().panel.scope = 0;
                h.run();
                assert_eq!(h.state().history.project(), &before);
                assert!(!h.state().history.can_undo());
            }
        }
        j_ui::i18n::set_language("en");
    }

    #[test]
    fn caption_edits_change_burned_program_export_frames_and_undo_restores_pixels() {
        if !j_ui::test_support::gpu() {
            return;
        }
        jvid_render::text::set_faces(j_ui::theme::CJK_FACES.map(|(_, bytes)| bytes).to_vec());
        let (device, queue) = crate::export::headless_gpu().unwrap();
        let mut renderer = crate::export::FrameRenderer::new(&device);
        let mut h = fixture();
        let frame = |renderer: &mut crate::export::FrameRenderer, project: &Project, at| {
            let texture = renderer.render(project, at, &device, &queue).unwrap();
            jvid_render::composite::read_back(&device, &queue, texture)
        };
        let before = frame(&mut renderer, h.project(), Time(S));
        let id = captions::rows(h.project())[0].clip.id;
        h.apply(Command::Caption(Edit::Row {
            id,
            text: "Updated 字幕".into(),
            start: Time(0),
            end: Time(S),
        }))
        .unwrap();
        let edited = frame(&mut renderer, h.project(), Time(S / 2));
        let after_end = frame(&mut renderer, h.project(), Time(S));
        assert_ne!(before, edited);
        assert_ne!(edited, after_end);
        assert!(
            edited
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0)
        );
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/caption-workspace");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("edited-burned-frame.png");
        crate::export::frame_png(h.project(), Time(S / 2), &device, &queue, &path).unwrap();
        let exported = image::open(&path).unwrap().into_rgba8().into_raw();
        assert_eq!(exported, edited);
        let track = captions::rows(h.project())[0].track.id;
        let mut settings = h.project().sequence().track(track).unwrap().settings();
        settings.hidden = true;
        h.apply(Command::SetTrack(settings)).unwrap();
        assert_eq!(frame(&mut renderer, h.project(), Time(S / 2)), after_end);
        assert!(h.project().subtitle_cues().is_empty());
        assert!(h.undo() && h.undo());
        assert_eq!(frame(&mut renderer, h.project(), Time(S)), before);
    }
}
