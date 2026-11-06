//! Mouse routes to the timeline's keymap actions: the Edit menu's rows, a toolbar in the
//! panel toolbar, and right-click menus on clips and empty lane space. Every one runs
//! [`Timeline::action`], the code its shortcut runs, so it undoes the same way.

use egui::{Rect, Ui, Vec2, pos2};
use jvid_core::model::{ClipId, Project, Sequence};
use suite_ui::theme::icon;
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::{Button, State};

use super::{Edit, Timeline, Tool, add_menu};
use crate::i18n::tr;
use crate::keymap::Keymap;

/// The app bar's File menu; "" is a separator, "recent" the Open Recent submenu and
/// "reframe" the Reframe submenu.
pub const FILE: &[&str] = &[
    "new_project",
    "open_project",
    "recent",
    "",
    "save",
    "save_as",
    "reframe",
    "",
    "import",
    "import_subtitles",
    "export_subtitles",
    "export",
    "",
    "quit",
];

/// The app bar's Edit menu; "" is a separator.
pub const EDIT: &[&str] = &[
    "undo",
    "redo",
    "",
    "cut",
    "copy",
    "paste",
    "duplicate",
    "copy_settings",
    "paste_settings",
    "",
    "delete",
    "ripple_delete",
    "add_edit",
    "ripple_trim_prev",
    "ripple_trim_next",
    "link",
    "",
    "mark_in",
    "mark_out",
    "mark_clip",
    "mark_selection",
    "clear_in_out",
    "lift",
    "extract",
    "",
    "add_marker",
    "add_range_marker",
    "",
    "select_all",
];

const CLIP: &[&str] = &[
    "cut",
    "copy",
    "paste",
    "duplicate",
    "",
    "delete",
    "ripple_delete",
    "add_edit",
    "link",
    "enable",
    "",
    "reverse",
    "freeze_frame",
    "detect_silence",
    "find_highlights",
    "",
    "copy_grade",
    "paste_grade",
    "hide_region",
    "clean_up_voice",
    "nest",
    "",
    "crossfade",
    "dip_to_black",
    "dip_to_white",
];

/// A separate 32pt column below the dock strip.
pub const RAIL_WIDTH: f32 = 32.0;

/// Editing tools, snap/link toggles, then zoom controls.
const TOOLBAR: &[(&str, &str)] = &[
    ("selection_tool", icon::POINTER),
    ("razor_tool", icon::SCISSORS),
    ("text_tool", ""),
    ("add_edit", icon::SPLIT),
    ("snapping", icon::MAGNET),
    ("linked_selection", icon::LINK),
    ("zoom_out", icon::ZOOM_OUT),
    ("zoom_in", icon::ZOOM_IN),
    ("zoom_fit", icon::SCAN),
];

/// A menu row for action `id` with its live shortcut, greyed out unless `enabled`.
pub fn item(ui: &mut Ui, keymap: &Keymap, id: &str, label: Option<&str>, enabled: bool) -> bool {
    let (name, shortcut) = keymap.label(id);
    let row = egui::Button::new(label.unwrap_or(name)).shortcut_text(shortcut);
    ui.add_enabled(enabled, row).clicked()
}

impl Timeline {
    /// Whether action `id` would change anything now; menus grey out the rest.
    pub fn can(&mut self, id: &str, seq: &Sequence) -> bool {
        if let Some(enabled) = self.key_capability(id, seq) {
            return enabled;
        }
        let any = !self.selected.is_empty();
        match id {
            "fit_to_frame" | "fill_frame" | "stretch_to_fit" => !self.video_chosen(seq).is_empty(),
            "paste" => !self.clipboard.is_empty(),
            "copy_grade" => !self.video_chosen(seq).is_empty(),
            "paste_grade" => self.grade.is_some() && self.clip_action(id, seq).is_some(),
            "delete" | "ripple_delete" => any || self.gap.is_some(),
            "copy" | "cut" | "duplicate" | "link" | "enable" | "reverse" | "nest" => any,
            "freeze_frame" | "clean_up_voice" => self.clip_action(id, seq).is_some(),
            "hide_region" => {
                !self.video_chosen(seq).is_empty() && crate::hide_region::blur().is_some()
            }
            "detect_silence" => super::silence::Silence::open(seq, &self.selected).is_some(),
            "find_highlights" => super::highlights::Highlights::open(seq, &self.selected).is_some(),
            // cut_action's only side effect is Q's seek, put back after.
            "add_edit" | "ripple_trim_prev" | "ripple_trim_next" => {
                let seek = self.seek_after;
                let can = self.cut_action(id, seq).is_some();
                self.seek_after = seek;
                can
            }
            "select_all" => seq.tracks().iter().any(|t| !t.clips().is_empty()),
            "clear_in_out" | "lift" | "extract" => {
                seq.in_point().is_some() || seq.out_point().is_some()
            }
            "add_range_marker" => seq.in_point().is_some() || seq.out_point().is_some(),
            "mark_clip" => seq.clips_at(self.playhead()).next().is_some(),
            "mark_selection" => any,
            _ => true,
        }
    }

    fn items(&mut self, ui: &mut Ui, seq: &Sequence, keymap: &Keymap, ids: &[&'static str]) {
        for &id in ids {
            if id.is_empty() {
                ui.separator();
            } else if item(ui, keymap, id, None, self.can(id, seq)) {
                self.menu = Some(id);
            }
        }
    }

    /// Right-click on a clip, which the caller has already selected.
    pub(super) fn clip_menu(
        &mut self,
        ui: &mut Ui,
        seq: &Sequence,
        keymap: &Keymap,
        project: &Project,
        clip: ClipId,
    ) -> Option<jvid_core::history::Command> {
        let (track, c) = seq.clip(clip).expect("clicked clip");
        if seq.track(track).is_some_and(|t| t.subtitles)
            && ui.button(tr!("action.export_subtitles")).clicked()
        {
            self.export_subtitles = true;
        }
        self.items(ui, seq, keymap, CLIP);
        ui.separator();
        let edit = crate::framing::menu(ui, project, Some(clip), self.playhead());
        ui.separator();
        if ui.button(tr!("timeline.reveal_in_bin")).clicked() {
            self.reveal = Some(c.asset);
        }
        edit
    }

    /// Right-click on empty lane space; the caller has picked the gap under it, if any.
    pub(super) fn lane_menu(
        &mut self,
        ui: &mut Ui,
        seq: &Sequence,
        keymap: &Keymap,
        edit: &mut Option<Edit>,
    ) {
        self.items(ui, seq, keymap, &["paste"]);
        if self.gap.is_some()
            && item(
                ui,
                keymap,
                "ripple_delete",
                Some(tr!("timeline.close_gap")),
                true,
            )
        {
            self.menu = Some("ripple_delete");
        }
        ui.separator();
        add_menu(ui, edit);
    }

    /// Compact rail, with an overflow menu when the panel is too short for every tool.
    pub fn tool_rail(&mut self, ui: &mut Ui, rail: Rect, project: &Project, keymap: &Keymap) {
        let line = 1.0 / ui.pixels_per_point();
        ui.painter().rect_filled(rail, 0.0, color::panel());
        ui.painter()
            .rect_filled(rail.with_min_x(rail.right() - line), 0.0, color::border());
        let step = size::CONTROL + space::S4;
        let sequences = project.sequences().len() > 1;
        let extra = if sequences { step } else { 0.0 };
        let full = TOOLBAR.len() as f32 * step + 2.0 * space::S8 + extra;
        let shown = if full <= rail.height() {
            TOOLBAR.len()
        } else {
            ((rail.height() - step - space::S4).max(0.0) / step) as usize
        }
        .min(TOOLBAR.len());
        let mut top = rail.top() + space::S4;
        for (i, &(id, glyph)) in TOOLBAR[..shown].iter().enumerate() {
            if shown == TOOLBAR.len() && matches!(i, 4 | 6) {
                top += space::S8;
                if i == 6 {
                    top = top.max(rail.bottom() - 3.0 * step - extra);
                }
                ui.painter().hline(
                    rail.left() + space::S4..=rail.right() - space::S4,
                    top - space::S4,
                    egui::Stroke::new(line, color::border()),
                );
            }
            let r = Rect::from_min_size(
                pos2(rail.left() + space::S4, top),
                Vec2::splat(size::CONTROL),
            );
            top += step;
            let on = match id {
                "selection_tool" => self.tool == Tool::Selection,
                "razor_tool" => self.tool == Tool::Razor,
                "text_tool" => self.tool == Tool::Text,
                "snapping" => self.snapping,
                "linked_selection" => self.linked_selection,
                _ => false,
            };
            let state = if self.can(id, project.sequence()) {
                State::Live
            } else {
                State::Disabled
            };
            let (name, shortcut) = keymap.label(id);
            let mut tip = if shortcut.is_empty() {
                name.to_owned()
            } else {
                format!("{name} ({shortcut})")
            };
            if id == "text_tool" {
                tip.push_str(&format!("\n{}", tr!("timeline.text_tool_hint")));
            }
            let b = Button::icon(glyph, name).on(on).state(state);
            let response = ui.put(r, b);
            if id == "text_tool" {
                ui.painter().text(
                    response.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    crate::i18n::verbatim("T"),
                    suite_ui::theme::body_strong(),
                    if on || response.hovered() {
                        color::ink()
                    } else {
                        color::ink_2()
                    },
                );
            }
            if response
                .on_hover_text(&tip)
                .on_disabled_hover_text(tip)
                .clicked()
            {
                self.action(id, project);
                ui.ctx().request_repaint();
            }
        }
        if (shown < TOOLBAR.len() || sequences) && rail.height() >= size::CONTROL {
            let top = (rail.bottom() - step).max(rail.top());
            let r = Rect::from_min_size(
                pos2(rail.left() + space::S4, top),
                Vec2::splat(size::CONTROL),
            );
            let overflow = ui.put(r, Button::icon(icon::ELLIPSIS, tr!("preview.more")));
            egui::Popup::menu(&overflow).show(|ui| {
                for &(id, _) in &TOOLBAR[shown..] {
                    let enabled = self.can(id, project.sequence());
                    if item(ui, keymap, id, None, enabled) {
                        self.action(id, project);
                    }
                }
                if sequences {
                    ui.separator();
                    ui.label(tr!("timeline.open_sequence"));
                    for seq in project.sequences() {
                        let name = project
                            .sequence_asset(seq.id())
                            .map(|a| a.display_name())
                            .unwrap_or_else(|| tr!("timeline.main_sequence").to_owned());
                        if ui
                            .selectable_label(seq.id() == project.sequence().id(), name)
                            .clicked()
                        {
                            self.open_sequence = Some(seq.id());
                            ui.close();
                        }
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton};
    use egui_kittest::kittest::Queryable;
    use jvid_core::history::History;
    use jvid_core::time::{FLICKS_PER_SECOND as S, Time};

    use super::super::tests::{harness, project};
    use super::*;
    use crate::keymap::{ACTIONS, Context};

    #[test]
    fn every_timeline_edit_is_in_a_menu_or_the_toolbar() {
        let p = project(4);
        let shown: Vec<&str> = EDIT
            .iter()
            .chain(CLIP)
            .copied()
            .chain(TOOLBAR.iter().map(|(id, _)| *id))
            .chain(crate::framing::ACTIONS)
            .collect();
        let tools = ["undo", "redo", "razor_tool", "selection_tool", "text_tool"];
        for a in ACTIONS {
            let ours = a.context == Context::Timeline || tools.contains(&a.id);
            // Lift and Extract are stubs until in/out lands on the timeline.
            if ours && (tools.contains(&a.id) || Timeline::default().action(a.id, &p)) {
                assert!(shown.contains(&a.id), "{} has no menu", a.id);
            }
        }
    }

    #[test]
    fn items_grey_out_when_they_cant_apply() {
        let p = project(4);
        let seq = p.sequence();
        let mut t = Timeline::default();
        t.sync(&p);
        for id in [
            "copy", "cut", "paste", "delete", "link", "enable", "add_edit",
        ] {
            assert!(!t.can(id, seq), "{id}");
        }
        assert!(!t.can("select_all", Project::default().sequence()));
        t.seek(Time(S));
        assert!(t.can("add_edit", seq));
        assert!(t.action("select_all", &p));
        assert_eq!(t.selected.len(), 4);
        assert!(t.can("copy", seq) && !t.can("paste", seq));
        t.action("copy", &p);
        assert!(t.can("paste", seq));
    }

    /// Right-click an unselected clip, pick Ripple Delete: the same edit and undo as the key.
    #[test]
    fn clip_menu_runs_the_shortcut_edit() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let p = project(4);
        let mut h = harness(p.clone());
        let lane_y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let at = pos2(8.0 + size::TRACK_HEADER + 20.0, lane_y);
        h.hover_at(at);
        h.run();
        for pressed in [true, false] {
            h.event(Event::PointerButton {
                pos: at,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            h.run();
        }
        let clip = p.sequence().tracks()[0].clips()[0].id;
        assert_eq!(h.state().0.selected().iter().collect::<Vec<_>>(), [&clip]);
        // Rows carry the live shortcut.
        h.get_by_label("Ripple Delete Shift+Delete").click();
        h.run();
        h.run();

        let mut key = History::new(p.clone());
        let mut t = Timeline::default();
        t.select(p.sequence(), clip, Modifiers::NONE);
        assert!(t.action("ripple_delete", &p));
        key.apply(t.pending.take().unwrap()).unwrap();
        assert_eq!(h.state().1.project(), key.project());
        assert!(h.state_mut().1.undo() && key.undo());
        assert_eq!(h.state().1.project(), &p);
    }
    #[test]
    fn framing_actions_run_the_shared_command_and_skip_ineligible_clips() {
        use jvid_core::framing::Framing;
        let (mut h, ids) = crate::framing::tests::fixture(2);
        let mut t = Timeline::default();
        t.action("select_all", h.project());
        for action in crate::framing::ACTIONS {
            assert!(t.action(action, h.project()));
            let cmd = t.pending.take().unwrap();
            let want = crate::framing::command(
                h.project(),
                &ids,
                crate::framing::mode(action).unwrap(),
                Time(0),
            )
            .unwrap();
            let mut expected = History::new(h.project().clone());
            expected.apply(want).unwrap();
            h.apply(cmd).unwrap();
            assert_eq!(h.project(), expected.project());
            assert!(h.undo());
        }
        let mut settings = h.project().sequence().tracks()[0].settings();
        settings.locked = true;
        h.apply(jvid_core::history::Command::SetTrack(settings))
            .unwrap();
        assert!(
            crate::framing::command(h.project(), &[ids[0]], Framing::Stretch, Time(0)).is_none()
        );
        let audio = project(2);
        let id = audio.sequence().tracks()[1].clips()[0].id;
        assert!(crate::framing::command(&audio, &[id], Framing::Stretch, Time(0)).is_none());
    }

    #[test]
    fn timeline_framing_context_menu_changes_only_the_clicked_clip() {
        if !suite_ui::test_support::gpu() {
            return;
        }
        let (history, ids) = crate::framing::tests::fixture(2);
        let mut h = harness(history.project().clone());
        h.set_size(Vec2::new(1000.0, 800.0));
        h.run();
        h.state_mut().0.action("select_all", history.project());
        h.run();
        let lane_y = h.get_all_by_label("Lock").next().unwrap().rect().center().y;
        let at = pos2(8.0 + size::TRACK_HEADER + 20.0, lane_y);
        h.hover_at(at);
        h.run();
        for pressed in [true, false] {
            h.event(Event::PointerButton {
                pos: at,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            h.run();
        }
        h.get_by_label("Fit to Frame");
        h.get_by_label("Fill Frame");
        h.get_by_label("Stretch to Fit").click();
        h.run();
        h.run();
        let p = h.state().1.project();
        assert_eq!(
            p.sequence().clip(ids[0]).unwrap().1.transform.scale,
            [4.0 / 3.0, 1.0]
        );
        assert_eq!(
            p.sequence().clip(ids[1]).unwrap().1.transform.scale,
            [1.0; 2]
        );
        assert!(h.state_mut().1.undo());
        assert_eq!(h.state().1.project(), history.project());
    }
}
