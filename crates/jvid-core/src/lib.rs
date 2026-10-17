//! Project model, timeline, keyframes, commands with undo, and serialization.
//!
//! This crate stays free of I/O and UI so it can be tested in isolation and
//! driven by both the desktop app and the headless CLI.

pub mod model;
pub mod time;
