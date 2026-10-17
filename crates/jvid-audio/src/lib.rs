//! Audio mixer, cpal output, resampling and loudness.

mod output;

pub use output::{Backend, CpalBackend, Feed, Output, SAMPLE_RATE};
