//! Project model, timeline, keyframes, commands with undo, and serialization.
//!
//! This crate stays free of I/O and UI so it can be tested in isolation and
//! driven by both the desktop app and the headless CLI.

pub mod eval;
pub mod file;
pub mod history;
pub mod keyframe;
pub mod model;
pub mod settings;
pub mod time;

#[allow(dead_code)]
fn gate_probe() -> bool {
    let x = 1;
    x == x
}
