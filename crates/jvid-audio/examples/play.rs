//! Plays a file's first audio stream and prints the device and master clock.
//! `cargo run -p jvid-audio --example play -- <file> [device name]`; with no file, lists devices.

use std::time::Duration;

use jvid_audio::{CpalBackend, Output};
use jvid_media::audio::AudioDecoder;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = Output::new(Box::<CpalBackend>::default(), args.get(1).cloned());
    let Some(path) = args.first() else {
        out.devices().iter().for_each(|d| println!("{d}"));
        return;
    };
    jvid_media::ffmpeg::init().expect("ffmpeg init");
    let mut dec = AudioDecoder::open(path.as_ref(), 0).expect("open audio");
    let mut pending: Vec<f32> = Vec::new();
    let mut tick = 0;
    loop {
        if pending.is_empty() {
            match dec.next_chunk().expect("decode") {
                Some(c) => pending = c.samples,
                None => break,
            }
        }
        let n = out.push(&pending);
        pending.drain(..n);
        out.poll();
        std::thread::sleep(Duration::from_millis(10));
        tick += 1;
        if tick % 50 == 0 {
            let secs = out.clock() as f64 / jvid_audio::SAMPLE_RATE as f64;
            println!("{:>8.2}s  {}", secs, out.device().unwrap_or("<wall clock>"));
        }
    }
}
