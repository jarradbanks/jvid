//! Docking-only workspaces. Panel identity stays independent of translated titles.
//!
//! Optional panels register their stable semantic ID beside their `app::Tab` variant.
//! A missing registration means unavailable: loading a layout omits that panel, never
//! substitutes another optional panel. An absent optional ID means closed and stays closed.
//! Snapshots must never own project history, jobs, transcript caches or panel domain state.

use crate::app::Tab;
use egui_dock::{DockState, Node, NodeIndex, Surface, TabIndex, Tree};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

/// Reserved for the captions consumer; never use a translated label as identity.
pub const CAPTIONS_PANEL_ID: &str = "captions";
/// Reserved for the highlights consumer; never use a translated label as identity.
pub const HIGHLIGHTS_PANEL_ID: &str = "highlights";

/// Additive registration contract for optional feature panels. Existing `Tab` variants
/// keep their serialized Rust names; workspace snapshots use these semantic IDs instead.
#[derive(Clone, Copy, Debug)]
pub struct PanelRegistration {
    pub id: &'static str,
    pub tab: Tab,
}

/// Final feature wiring adds registrations here through the shell ownership window.
/// Unregistered IDs in saved workspaces are safely omitted when applying the layout.
pub const OPTIONAL_PANELS: &[PanelRegistration] = &[PanelRegistration {
    id: HIGHLIGHTS_PANEL_ID,
    tab: Tab::Highlights,
}, PanelRegistration {
    id: CAPTIONS_PANEL_ID,
    tab: Tab::Captions,
}];

const CORE_PANELS: &[PanelRegistration] = &[
    PanelRegistration {
        id: "media",
        tab: Tab::Media,
    },
    PanelRegistration {
        id: "preview",
        tab: Tab::Preview,
    },
    PanelRegistration {
        id: "inspector",
        tab: Tab::Inspector,
    },
    PanelRegistration {
        id: "timeline",
        tab: Tab::Timeline,
    },
];

const EXISTING_OPTIONAL_PANELS: &[PanelRegistration] = &[
    PanelRegistration {
        id: "render_queue",
        tab: Tab::RenderQueue,
    },
    PanelRegistration {
        id: "transcript",
        tab: Tab::Transcript,
    },
];

/// Available panel identities for this application build. Consumers supply only their
/// available optional registrations; core registrations always remain authoritative.
#[derive(Clone, Copy)]
pub struct Panels<'a>(pub &'a [PanelRegistration]);

impl Default for Panels<'static> {
    fn default() -> Self {
        Self(OPTIONAL_PANELS)
    }
}

impl Panels<'_> {
    fn registrations(self) -> impl Iterator<Item = PanelRegistration> {
        CORE_PANELS
            .iter()
            .chain(EXISTING_OPTIONAL_PANELS)
            .chain(self.0)
            .copied()
    }

    pub fn tab(self, id: &str) -> Option<Tab> {
        self.registrations()
            .find(|panel| panel.id == id)
            .map(|panel| panel.tab)
    }

    pub fn id(self, tab: Tab) -> Option<&'static str> {
        self.registrations()
            .find(|panel| panel.tab == tab)
            .map(|panel| panel.id)
    }
}

/// Only panel placement and focus; strings allow unknown optional panels to deserialize.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    dock: DockState<String>,
    #[serde(default)]
    pub(crate) focused: Option<String>,
}

impl Snapshot {
    pub fn capture(dock: &DockState<Tab>, panels: Panels<'_>) -> Self {
        let mut copy = dock.clone();
        let focused = copy
            .find_active_focused()
            .and_then(|(_, tab)| panels.id(*tab));
        Self {
            dock: map_dock(dock, |tab| panels.id(*tab).map(str::to_owned)),
            focused: focused.map(str::to_owned),
        }
    }

    /// Read legacy `DockState<Tab>` without trusting its persisted focus indices.
    pub fn legacy(dock: &DockState<Tab>, panels: Panels<'_>) -> Self {
        if valid_dock(dock) {
            Self {
                dock: map_dock(dock, |tab| panels.id(*tab).map(str::to_owned)),
                focused: None,
            }
        } else {
            let mut repaired = editing();
            for (_, tab) in dock.iter_all_tabs() {
                if panels.id(*tab).is_some() && repaired.find_tab(tab).is_none() {
                    repaired.push_to_first_leaf(*tab);
                }
            }
            Self::capture(&repaired, panels)
        }
    }

    pub fn restore(&self, panels: Panels<'_>) -> DockState<Tab> {
        let mut seen = std::collections::HashSet::new();
        let mut dock = if valid_dock(&self.dock) {
            map_dock(&self.dock, |id| {
                panels.tab(id).filter(|tab| seen.insert(*tab))
            })
        } else {
            // Invalid topology cannot be trusted, but recognized optional panels survive.
            let mut dock = editing();
            for (_, id) in self.dock.iter_all_tabs() {
                if let Some(tab) = panels.tab(id)
                    && dock.find_tab(&tab).is_none()
                {
                    dock.push_to_first_leaf(tab);
                }
            }
            dock
        };
        for panel in CORE_PANELS {
            if dock.find_tab(&panel.tab).is_none() {
                dock.push_to_first_leaf(panel.tab);
            }
        }
        for (_, node) in dock.iter_all_nodes_mut() {
            if !node.is_empty() {
                node.set_collapsed(false);
            }
            match node {
                Node::Leaf(leaf) => {
                    leaf.tab_bar_hidden = false;
                    leaf.active = TabIndex(leaf.active.0.min(leaf.tabs.len().saturating_sub(1)));
                    if !leaf.scroll.is_finite() {
                        leaf.scroll = 0.0;
                    }
                }
                Node::Horizontal(split) | Node::Vertical(split) => {
                    split.fraction = if split.fraction.is_finite() {
                        split.fraction.clamp(0.05, 0.95)
                    } else {
                        0.5
                    };
                    split.collapsed_leaf_count = 0;
                }
                Node::Empty => {}
            }
        }
        if let Some(id) = self.focused.as_deref() {
            let focused = panels
                .tab(id)
                .filter(|tab| dock.find_tab(tab).is_some())
                .unwrap_or(Tab::Timeline);
            focus(&mut dock, focused);
        }
        dock
    }
}

// Map surfaces individually: egui_dock::filter_map_tabs can discard an empty main
// surface and retain stale surface focus indices after removing unavailable windows.
fn map_dock<T, U>(dock: &DockState<T>, mut map: impl FnMut(&T) -> Option<U>) -> DockState<U> {
    let mut result = DockState::new(Vec::new());
    for surface in dock.iter_surfaces() {
        match surface {
            Surface::Main(tree) => *result.main_surface_mut() = map_tree(tree, &mut map),
            Surface::Window(tree, state) => {
                let tree = map_tree(tree, &mut map);
                if tree.num_tabs() > 0 {
                    let index = result.add_window(Vec::new());
                    result[index] = tree;
                    *result.get_window_state_mut(index).expect("new window") = state.clone();
                }
            }
            Surface::Empty => {}
        }
    }
    result
}

fn map_tree<T, U>(tree: &Tree<T>, map: impl FnMut(&T) -> Option<U>) -> Tree<U> {
    let mut tree = tree.filter_map_tabs(map);
    if let Some(index) = tree.iter().position(|node| node.is_leaf()) {
        tree.set_focused_node(NodeIndex(index));
    }
    tree
}

fn valid_dock<T>(dock: &DockState<T>) -> bool {
    matches!(dock.iter_surfaces().next(), Some(Surface::Main(_)))
        && dock.iter_surfaces().all(|surface| match surface {
            Surface::Main(tree) | Surface::Window(tree, _) => valid_tree(tree),
            Surface::Empty => true,
        })
}

fn valid_tree<T>(tree: &Tree<T>) -> bool {
    let nodes: Vec<_> = tree.iter().collect();
    nodes.iter().enumerate().all(|(index, node)| {
        if node.is_empty() {
            return true;
        }
        if index > 0 && !nodes[(index - 1) / 2].is_parent() {
            return false;
        }
        if node.is_parent() {
            [index * 2 + 1, index * 2 + 2]
                .into_iter()
                .all(|child| nodes.get(child).is_some_and(|node| !node.is_empty()))
        } else {
            true
        }
    })
}

fn focus(dock: &mut DockState<Tab>, tab: Tab) {
    if let Some(path) = dock.find_tab(&tab) {
        let _ = dock.set_active_tab(path);
        dock.set_focused_node_and_surface(path.node_path());
    }
}

fn editing() -> DockState<Tab> {
    crate::app::default_dock()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Editing,
    Captions,
    Audio,
}

impl Builtin {
    pub fn layout(self, panels: Panels<'_>) -> DockState<Tab> {
        let mut dock = match self {
            Self::Editing => editing(),
            Self::Captions => {
                let mut dock = editing();
                let path = dock.find_tab(&Tab::Media).expect("media");
                let tab = panels.tab(CAPTIONS_PANEL_ID).unwrap_or(Tab::Transcript);
                dock[path.node_path()].append_tab(tab);
                dock
            }
            Self::Audio => {
                let mut dock = DockState::new(vec![Tab::Preview, Tab::Media]);
                let tree = dock.main_surface_mut();
                let [top, _] = tree.split_below(NodeIndex::root(), 0.38, vec![Tab::Timeline]);
                tree.split_right(top, 0.65, vec![Tab::Inspector]);
                dock
            }
        };
        focus(
            &mut dock,
            if self == Self::Captions {
                panels.tab(CAPTIONS_PANEL_ID).unwrap_or(Tab::Transcript)
            } else {
                Tab::Timeline
            },
        );
        dock
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Personal {
    pub name: String,
    pub layout: Snapshot,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspaces {
    pub personal: Vec<Personal>,
    /// Transient maximize state; persisted layout always uses this full backup.
    #[serde(skip)]
    normal: Option<DockState<Tab>>,
    #[serde(skip)]
    dialog: Option<WorkspaceDialog>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    Duplicate,
    Missing,
}

impl Workspaces {
    pub fn normal_dock<'a>(&'a self, dock: &'a DockState<Tab>) -> &'a DockState<Tab> {
        self.normal.as_ref().unwrap_or(dock)
    }

    pub fn is_maximized(&self) -> bool {
        self.normal.is_some()
    }

    pub fn restore_maximized(&mut self, dock: &mut DockState<Tab>) -> bool {
        if let Some(normal) = self.normal.take() {
            *dock = normal;
            true
        } else {
            false
        }
    }

    pub fn toggle_maximize(&mut self, dock: &mut DockState<Tab>) {
        if self.restore_maximized(dock) {
            return;
        }
        if let Some(tab) = dock.find_active_focused().map(|(_, tab)| *tab) {
            self.normal = Some(std::mem::replace(dock, DockState::new(vec![tab])));
            focus(dock, tab);
        }
    }

    pub fn apply(&mut self, dock: &mut DockState<Tab>, layout: DockState<Tab>) {
        self.normal = None;
        *dock = layout;
    }

    pub fn reset(&mut self, dock: &mut DockState<Tab>, panels: Panels<'_>) {
        let optional: Vec<_> = self
            .normal_dock(dock)
            .iter_all_tabs()
            .filter_map(|(_, tab)| (!CORE_PANELS.iter().any(|p| p.tab == *tab)).then_some(*tab))
            .filter(|tab| panels.id(*tab).is_some())
            .collect();
        let mut layout = editing();
        for tab in optional {
            if layout.find_tab(&tab).is_none() {
                let path = layout.find_tab(&Tab::Timeline).expect("timeline");
                layout
                    .main_surface_mut()
                    .split_right(path.node, 0.5, vec![tab]);
            }
        }
        focus(&mut layout, Tab::Timeline);
        self.apply(dock, layout);
    }

    fn name(&self, name: &str, except: Option<usize>) -> Result<String, NameError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(NameError::Empty);
        }
        if self
            .personal
            .iter()
            .enumerate()
            .any(|(index, p)| Some(index) != except && p.name.to_lowercase() == name.to_lowercase())
        {
            return Err(NameError::Duplicate);
        }
        Ok(name.to_owned())
    }

    pub fn save(
        &mut self,
        name: &str,
        dock: &DockState<Tab>,
        panels: Panels<'_>,
    ) -> Result<(), NameError> {
        let name = self.name(name, None)?;
        let layout = Snapshot::capture(self.normal_dock(dock), panels);
        self.personal.push(Personal { name, layout });
        Ok(())
    }

    pub fn rename(&mut self, index: usize, name: &str) -> Result<(), NameError> {
        if index >= self.personal.len() {
            return Err(NameError::Missing);
        }
        let name = self.name(name, Some(index))?;
        self.personal[index].name = name;
        Ok(())
    }

    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.personal.len() {
            return false;
        }
        self.personal.remove(index);
        true
    }
}

struct WorkspaceDialog {
    rename: Option<usize>,
    name: String,
    error: Option<NameError>,
    focus_name: bool,
}

impl Default for WorkspaceDialog {
    fn default() -> Self {
        Self {
            rename: None,
            name: String::new(),
            error: None,
            focus_name: true,
        }
    }
}

impl Workspaces {
    pub fn open_save(&mut self) {
        self.dialog = Some(WorkspaceDialog::default());
    }

    pub fn open_rename(&mut self, index: usize) {
        if let Some(personal) = self.personal.get(index) {
            self.dialog = Some(WorkspaceDialog {
                rename: Some(index),
                name: personal.name.clone(),
                ..Default::default()
            });
        }
    }

    pub fn is_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub fn show(&mut self, ui: &mut egui::Ui, dock: &DockState<Tab>, panels: Panels<'_>) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let title = if dialog.rename.is_some() {
            tr!("workspace.rename")
        } else {
            tr!("workspace.save")
        };
        let mut accept = false;
        let mut cancel = false;
        let result = egui::Modal::new(egui::Id::new("workspace_name")).show(ui.ctx(), |ui| {
            ui.set_width(320.0_f32.min((ui.ctx().content_rect().width() - 48.0).max(1.0)));
            ui.heading(title);
            let input = ui.add(
                egui::TextEdit::singleline(&mut dialog.name)
                    .hint_text(tr!("workspace.name"))
                    .desired_width(f32::INFINITY),
            );
            if dialog.focus_name {
                input.request_focus();
                dialog.focus_name = false;
            }
            if let Some(error) = dialog.error {
                ui.colored_label(
                    suite_ui::tokens::color::red(),
                    match error {
                        NameError::Empty => tr!("workspace.empty_name"),
                        NameError::Duplicate => tr!("workspace.duplicate_name"),
                        NameError::Missing => tr!("workspace.missing"),
                    },
                );
            }
            let buttons = suite_ui::dialog::action_footer(
                ui,
                [
                    suite_ui::widgets::Button::secondary(tr!("prompt.cancel")),
                    suite_ui::widgets::Button::primary(title),
                ],
            );
            cancel = buttons[0].clicked();
            accept = buttons[1].clicked()
                || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        });
        if cancel || result.should_close() {
            return;
        }
        if accept {
            let result = match dialog.rename {
                Some(index) => self.rename(index, &dialog.name),
                None => self.save(&dialog.name, dock, panels),
            };
            match result {
                Ok(()) => return,
                Err(error) => dialog.error = Some(error),
            }
        }
        self.dialog = Some(dialog);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(dock: &DockState<Tab>) -> std::collections::HashSet<Tab> {
        dock.iter_all_tabs().map(|(_, tab)| *tab).collect()
    }

    fn encoded<T: Serialize>(value: &T) -> String {
        #[derive(Default)]
        struct Storage(String);
        impl eframe::Storage for Storage {
            fn get_string(&self, _: &str) -> Option<String> {
                Some(self.0.clone())
            }
            fn set_string(&mut self, _: &str, value: String) {
                self.0 = value;
            }
            fn remove_string(&mut self, _: &str) {}
            fn flush(&mut self) {}
        }
        let mut storage = Storage::default();
        eframe::set_value(&mut storage, "test", value);
        storage.0
    }

    #[test]
    fn builtins_keep_every_core_panel_and_captions_falls_back_to_transcript() {
        for builtin in [Builtin::Editing, Builtin::Captions, Builtin::Audio] {
            let dock = builtin.layout(Panels(&[]));
            for panel in CORE_PANELS {
                assert_eq!(
                    dock.iter_all_tabs()
                        .filter(|(_, tab)| **tab == panel.tab)
                        .count(),
                    1
                );
            }
            assert_eq!(
                tabs(&dock).contains(&Tab::Transcript),
                builtin == Builtin::Captions
            );
            assert!(!tabs(&dock).contains(&Tab::RenderQueue));
        }
        assert_eq!(CAPTIONS_PANEL_ID, "captions");
        assert_eq!(HIGHLIGHTS_PANEL_ID, "highlights");
        assert!(Panels(&[]).tab(CAPTIONS_PANEL_ID).is_none());
        assert_eq!(
            Panels::default().tab(CAPTIONS_PANEL_ID),
            Some(Tab::Captions)
        );
        assert!(tabs(&Builtin::Captions.layout(Panels::default())).contains(&Tab::Captions));
    }

    #[test]
    fn valid_legacy_layout_keeps_topology_and_optional_panels_stay_closed() {
        let mut dock = editing();
        let inspector = dock.find_tab(&Tab::Inspector).unwrap();
        dock.remove_tab(inspector);
        let timeline = dock.find_tab(&Tab::Timeline).unwrap();
        dock[timeline.node_path()].append_tab(Tab::Inspector);
        let restored = Snapshot::legacy(&dock, Panels::default()).restore(Panels::default());
        for tab in CORE_PANELS {
            assert_eq!(dock.find_tab(&tab.tab), restored.find_tab(&tab.tab));
        }
        assert_eq!(tabs(&restored), tabs(&dock));
    }

    #[test]
    fn missing_duplicate_and_unavailable_panels_repair_without_reopening_closed_panels() {
        let snapshot = Snapshot {
            dock: DockState::new(
                [
                    "preview",
                    "media",
                    "media",
                    "transcript",
                    "captions",
                    "highlights",
                    "future_panel",
                ]
                .map(str::to_owned)
                .to_vec(),
            ),
            focused: Some("captions".into()),
        };
        let mut restored = snapshot.restore(Panels(&[]));
        assert_eq!(restored.iter_all_tabs().count(), 5);
        for panel in CORE_PANELS {
            assert!(tabs(&restored).contains(&panel.tab));
        }
        assert!(tabs(&restored).contains(&Tab::Transcript));
        assert!(!tabs(&restored).contains(&Tab::RenderQueue));
        assert_eq!(
            restored.find_active_focused().map(|(_, tab)| *tab),
            Some(Tab::Timeline)
        );
        let snapshot = Snapshot {
            dock: DockState::new(vec![CAPTIONS_PANEL_ID.into()]),
            focused: None,
        };
        assert_eq!(snapshot.restore(Panels(&[])).iter_all_tabs().count(), 4);
    }

    #[test]
    fn unavailable_floating_panel_and_empty_main_surface_are_safe() {
        let mut raw = DockState::new(vec!["captions".into()]);
        raw.add_window(vec!["highlights".into()]);
        raw.add_window(vec!["timeline".into(), "transcript".into()]);
        let mut dock = Snapshot {
            dock: raw,
            focused: Some("highlights".into()),
        }
        .restore(Panels(&[]));
        assert_eq!(dock.iter_all_tabs().count(), 5);
        assert_eq!(
            dock.find_active_focused().map(|(_, tab)| *tab),
            Some(Tab::Timeline)
        );
    }

    #[test]
    fn invalid_topology_and_leaf_indices_do_not_panic_or_drop_optional_panels() {
        let mut raw = DockState::new(vec!["transcript".into()]);
        raw.main_surface_mut()
            .split_right(NodeIndex::root(), 0.5, vec!["preview".into()]);
        raw.main_surface_mut()[NodeIndex(2)] = Node::Empty;
        let snapshot = Snapshot {
            dock: raw,
            focused: None,
        };
        let restored = snapshot.restore(Panels::default());
        assert_eq!(restored.iter_all_tabs().count(), 5);
        assert!(tabs(&restored).contains(&Tab::Transcript));

        let mut dock = editing();
        for (_, leaf) in dock.iter_leaves_mut() {
            leaf.active = TabIndex(usize::MAX);
        }
        let restored = Snapshot::legacy(&dock, Panels::default()).restore(Panels::default());
        assert!(
            restored
                .iter_leaves()
                .all(|(_, leaf)| leaf.active.0 < leaf.tabs.len())
        );
    }

    #[test]
    fn maximize_restores_exact_layout_focus_and_normal_snapshot() {
        let panels = Panels::default();
        let mut dock = Builtin::Captions.layout(panels);
        focus(&mut dock, Tab::Inspector);
        let before = encoded(&dock);
        let mut workspaces = Workspaces::default();
        workspaces.toggle_maximize(&mut dock);
        assert!(workspaces.is_maximized());
        assert_eq!(tabs(&dock), [Tab::Inspector].into());
        assert_eq!(encoded(workspaces.normal_dock(&dock)), before);
        workspaces.save("Mine", &dock, panels).unwrap();
        assert_eq!(
            tabs(&workspaces.personal[0].layout.restore(panels)).len(),
            5
        );
        workspaces.toggle_maximize(&mut dock);
        assert_eq!(encoded(&dock), before);
        assert_eq!(
            dock.find_active_focused().map(|(_, tab)| *tab),
            Some(Tab::Inspector)
        );
    }

    #[test]
    fn personal_names_validate_and_rename_delete_keep_snapshots() {
        let mut workspaces = Workspaces::default();
        let dock = editing();
        let panels = Panels::default();
        assert_eq!(workspaces.save("  ", &dock, panels), Err(NameError::Empty));
        workspaces.save(" Mine ", &dock, panels).unwrap();
        assert_eq!(workspaces.personal[0].name, "Mine");
        assert_eq!(
            workspaces.save("mine", &dock, panels),
            Err(NameError::Duplicate)
        );
        workspaces.save("Other", &dock, panels).unwrap();
        assert_eq!(workspaces.rename(1, "MINE"), Err(NameError::Duplicate));
        workspaces.rename(0, "MINE").unwrap();
        let before = encoded(&workspaces.personal[1].layout);
        workspaces.rename(1, "Audio cuts").unwrap();
        assert_eq!(encoded(&workspaces.personal[1].layout), before);
        assert_eq!(workspaces.rename(2, "Missing"), Err(NameError::Missing));
        assert!(workspaces.delete(0));
        assert_eq!(workspaces.personal[0].name, "Audio cuts");
        assert!(!workspaces.delete(1));
    }

    #[test]
    fn reset_from_maximize_preserves_only_previously_open_optional_panels() {
        let panels = Panels::default();
        let mut dock = Builtin::Captions.layout(panels);
        let mut workspaces = Workspaces::default();
        workspaces.toggle_maximize(&mut dock);
        workspaces.reset(&mut dock, panels);
        assert!(!workspaces.is_maximized());
        assert_eq!(dock.iter_all_tabs().count(), 5);
        assert!(tabs(&dock).contains(&Tab::Captions));
        assert!(!tabs(&dock).contains(&Tab::RenderQueue));
        assert_eq!(
            dock.find_active_focused().map(|(_, tab)| *tab),
            Some(Tab::Timeline)
        );
        workspaces.apply(&mut dock, Builtin::Editing.layout(panels));
        workspaces.reset(&mut dock, panels);
        assert_eq!(dock.iter_all_tabs().count(), 4);
    }
}
