//! Mouse routes to the timeline's keymap actions: the Edit menu's rows, a toolbar in the
//! panel header, and right-click menus on clips and empty lane space. Every one runs
//! [`Timeline::action`], the code its shortcut runs, so it undoes the same way.

use egui::{Rect, Ui, Vec2, pos2};
use jvid_core::model::{AssetId, Project, Sequence};
use suite_ui::theme::icon;
use suite_ui::tokens::{size, space};
use suite_ui::widgets::{Button, Dropdown, State};

use super::{Edit, Timeline, add_menu};
use crate::i18n::tr;
use crate::keymap::Keymap;

/// The app bar's File menu; "" is a separator and "recent" the Open Recent submenu.
pub const FILE: &[&str] = &[
    "new_project",
    "open_project",
    "recent",
    "",
    "save",
    "save_as",
    "",
    "import",
    "import_subtitles",
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
    "",
    "copy_grade",
    "paste_grade",
    "clean_up_voice",
    "nest",
    "",
    "crossfade",
    "dip_to_black",
    "dip_to_white",
];

/// Header toolbar, left to right.
const TOOLBAR: &[(&str, &str)] = &[
    ("selection_tool", icon::POINTER),
    ("razor_tool", icon::SCISSORS),
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
        let any = !self.selected.is_empty();
        match id {
            "paste" => !self.clipboard.is_empty(),
            "copy_grade" => !self.video_chosen(seq).is_empty(),
            "paste_grade" => self.grade.is_some() && self.clip_action(id, seq).is_some(),
            "delete" | "ripple_delete" => any || self.gap.is_some(),
            "copy" | "cut" | "duplicate" | "link" | "enable" | "reverse" | "nest" => any,
            "freeze_frame" | "clean_up_voice" => self.clip_action(id, seq).is_some(),
            "detect_silence" => super::silence::Silence::open(seq, &self.selected).is_some(),
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
        asset: AssetId,
    ) {
        self.items(ui, seq, keymap, CLIP);
        ui.separator();
        if ui.button(tr!("timeline.reveal_in_bin")).clicked() {
            self.reveal = Some(asset);
        }
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

    /// Tool, snapping, linking, split and zoom buttons left-aligned in the panel `header`,
    /// starting at `left` (just past the title).
    pub fn toolbar(
        &mut self,
        ui: &mut Ui,
        header: Rect,
        left: f32,
        project: &Project,
        keymap: &Keymap,
    ) {
        let mut left = left;
        for &(id, glyph) in TOOLBAR {
            let y = header.center().y - size::CONTROL / 2.0;
            let r = Rect::from_min_size(pos2(left, y), Vec2::splat(size::CONTROL));
            left = r.right() + space::S4;
            let on = match id {
                "selection_tool" => !self.razor,
                "razor_tool" => self.razor,
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
            let tip = if shortcut.is_empty() {
                name.to_owned()
            } else {
                format!("{name} ({shortcut})")
            };
            let b = Button::icon(glyph, name).on(on).state(state);
            if ui
                .put(r, b)
                .on_hover_text(&tip)
                .on_disabled_hover_text(tip)
                .clicked()
            {
                // The edit, if any, goes out with the next `show`.
                self.action(id, project);
                ui.ctx().request_repaint();
            }
        }
        // Which sequence the timeline shows, once Nest has made a second.
        let seqs = project.sequences();
        if seqs.len() > 1 {
            let names: Vec<String> = seqs
                .iter()
                .map(|s| match project.sequence_asset(s.id()) {
                    Some(a) => a.display_name(),
                    None => tr!("timeline.main_sequence").to_owned(),
                })
                .collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let open = project.sequence().id();
            let mut i = seqs.iter().position(|s| s.id() == open).unwrap_or(0);
            // Up to the header's end, and left out where too little of it is left.
            let w = f32::min(160.0, header.right() - space::S8 - (left + space::S8));
            let y = header.center().y - size::CONTROL / 2.0;
            let r = Rect::from_min_size(pos2(left + space::S8, y), Vec2::new(w, size::CONTROL));
            if w >= 3.0 * size::CONTROL
                && ui
                    .put(r, Dropdown::new(&mut i, &names).width(w))
                    .on_hover_text(tr!("timeline.open_sequence"))
                    .changed()
            {
                self.open_sequence = Some(seqs[i].id());
            }
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
            .collect();
        let tools = ["undo", "redo", "razor_tool", "selection_tool"];
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
}
