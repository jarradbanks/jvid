//! Audio mixer, cpal output, resampling and loudness.

mod mixer;
mod output;

pub use mixer::{Meters, Mixer, RAMP_FRAMES, Strip, audible, pan_gains};
pub use output::{Backend, CpalBackend, Feed, Output, SAMPLE_RATE};
