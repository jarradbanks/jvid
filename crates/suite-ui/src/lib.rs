//! Shared theme, widgets and keybinding system, reused by every app in the suite.
//!
//! Call [`theme::install`] once at startup, then build panels with [`panel`] and
//! controls from [`widgets`]. Every colour, size and radius comes from [`tokens`].

pub mod dialog;
pub mod i18n;
pub mod panel;
#[cfg(feature = "test-support")]
pub mod test_support;
pub mod theme;
pub mod themes;
pub mod tokens;
pub mod widgets;
