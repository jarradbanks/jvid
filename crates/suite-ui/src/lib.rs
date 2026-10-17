//! Shared theme, widgets and keybinding system, reused by every app in the suite.
//!
//! Call [`theme::install`] once at startup, then build panels with [`panel`] and
//! controls from [`widgets`]. Every colour, size and radius comes from [`tokens`].

pub mod panel;
pub mod theme;
pub mod tokens;
pub mod widgets;
