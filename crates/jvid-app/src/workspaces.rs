//! Docking-only workspaces. Panel identity stays independent of translated titles.
//!
//! Optional panels register their stable semantic ID beside their `app::Tab` variant.
//! A missing registration means unavailable: loading a layout omits that panel, never
//! substitutes another optional panel. An absent optional ID means closed and stays closed.
//! Snapshots must never own project history, jobs, transcript caches or panel domain state.

use crate::app::Tab;

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
pub const OPTIONAL_PANELS: &[PanelRegistration] = &[];

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
