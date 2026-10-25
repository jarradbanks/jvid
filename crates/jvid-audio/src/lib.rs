//! Audio mixer, cpal output and input, resampling and loudness.

mod fx;
mod input;
mod loudness;
mod mixer;
mod output;

pub use fx::Fx;
pub use input::{Capture, CpalInput, Format, InputBackend, PEAK_FRAMES, Recorder};
pub use loudness::{Limiter, Meter};
pub use mixer::{Meters, Mixer, RAMP_FRAMES, Strip, audible, automate, balance, pan_gains};
pub use output::{Backend, CpalBackend, Feed, Output, SAMPLE_RATE};
