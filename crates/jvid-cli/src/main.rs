//! Headless render: turns a `.jvid` project into a video file without opening a window.
//!
//! `jvid render project.jvid -o out.mp4 [--preset NAME] [--in TC --out TC | --range IN-OUT]
//! [--loudness LUFS] [--target-size MB] [--lossless | --smart] [--json]`. Exports through the app's own export path, so the same project,
//! range and preset give the same file.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use jvid_app::export::{self, Error, Lossless, Request};
use jvid_core::model::Project;
use jvid_core::preset::{Preset, VideoCodec};
use jvid_core::time::{Time, parse_timecode};

const USAGE: &str = "usage: jvid render PROJECT.jvid -o OUT [--preset NAME] \
[--in TC --out TC | --range IN-OUT] [--loudness LUFS] [--target-size MB] \
[--lossless | --smart] [--json]";

/// Exit codes: 0 done, 1 export failed, 2 bad arguments, 3 project unreadable, 4 no GPU.
fn main() -> ExitCode {
    let _log = jvid_log::init("jvid-cli", jvid_log::log_dir());
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&args) {
        Ok(a) => a,
        Err(e) => return fail(2, &format!("{e}\n{USAGE}")),
    };
    let project = match jvid_app::project_file::load(&args.project) {
        Ok(p) => p,
        Err(e) => return fail(3, &format!("couldn't open {}: {e}", args.project.display())),
    };
    let req = match request(&project, &args, export::presets()) {
        Ok(r) => r,
        Err(e) => return fail(2, &e),
    };
    let gpu = match export::headless_gpu() {
        Ok(g) => g,
        Err(e) => return fail(4, &e),
    };
    let pool = jvid_jobs::Pool::new(1);
    let (handle, rx) = export::submit(&pool, project, req, gpu);
    let result = loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(r) => break r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                progress(handle.progress(), args.json)
            }
            Err(e) => break Err(Error::Failed(e.to_string())),
        }
    };
    match result {
        Ok(_) => {
            progress(1.0, args.json);
            if !args.json {
                eprintln!();
            }
            ExitCode::SUCCESS
        }
        Err(Error::Cancelled) => fail(1, "export cancelled"),
        Err(Error::Failed(e)) => fail(1, &e),
    }
}

fn progress(p: f32, json: bool) {
    if json {
        eprintln!("{{\"progress\":{p:.4}}}");
    } else {
        eprint!("\rrendering {:5.1}%", p * 100.0);
    }
}

fn fail(code: u8, msg: &str) -> ExitCode {
    eprintln!("\njvid: {msg}");
    ExitCode::from(code)
}

#[derive(Debug, Default, PartialEq)]
struct Args {
    project: PathBuf,
    output: PathBuf,
    preset: Option<String>,
    range: (Option<String>, Option<String>),
    loudness: Option<f64>,
    /// Megabytes, 10^6 bytes each.
    target_size: Option<u64>,
    /// `--lossless` copies the sources; `--smart` also re-encodes cut edges for exact frames.
    lossless: Option<Lossless>,
    json: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut it = args.iter();
    if it.next().map(String::as_str) != Some("render") {
        return Err("expected the `render` command".into());
    }
    let mut a = Args::default();
    let mut project = None;
    let mut output = None;
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "-o" | "--output" => output = Some(value()?),
            "--preset" => a.preset = Some(value()?),
            "--in" => a.range.0 = Some(value()?),
            "--out" => a.range.1 = Some(value()?),
            "--range" => {
                let v = value()?;
                let (i, o) = v.split_once('-').ok_or("--range wants IN-OUT")?;
                a.range = (Some(i.into()), Some(o.into()));
            }
            "--loudness" => {
                let v = value()?;
                a.loudness = Some(v.parse().map_err(|_| format!("bad loudness {v}"))?);
            }
            "--target-size" => {
                let v = value()?;
                let mb = v.parse().ok().filter(|&mb| mb > 0);
                a.target_size = Some(mb.ok_or(format!("bad target size {v}, want whole MB"))?);
            }
            "--lossless" => a.lossless = Some(Lossless::Copy),
            "--smart" => a.lossless = Some(Lossless::Smart),
            "--json" => a.json = true,
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s if project.is_none() => project = Some(s.to_string()),
            s => return Err(format!("unexpected argument {s}")),
        }
    }
    a.project = project.ok_or("missing project path")?.into();
    a.output = output.ok_or("missing -o OUT")?.into();
    Ok(a)
}

/// The export request, with the app's defaults: the first preset and the sequence's in/out points.
fn request(project: &Project, args: &Args, presets: Vec<Preset>) -> Result<Request, String> {
    let mut preset = match &args.preset {
        Some(n) => presets.into_iter().find(|p| &p.name == n),
        None => presets.into_iter().next(),
    }
    .ok_or_else(|| format!("no preset {:?}", args.preset.as_deref().unwrap_or_default()))?;
    if let Some(mb) = args.target_size {
        if !matches!(preset.video, Some(VideoCodec::H264 | VideoCodec::Hevc)) {
            return Err(format!(
                "--target-size needs an H.264 or HEVC preset, not {:?}",
                preset.name
            ));
        }
        let bytes = mb * 1_000_000;
        preset
            .options
            .insert("target_size".into(), bytes.to_string());
    }
    let seq = project.sequence();
    let rate = project.settings().frame_rate;
    let tc = |s: &Option<String>, default: Time| match s {
        Some(s) => parse_timecode(s, rate)
            .map(|f| rate.time_floor(f as i64))
            .ok_or(format!("bad timecode {s}, want HH:MM:SS:FF")),
        None => Ok(default),
    };
    let range = (
        tc(&args.range.0, seq.in_point().unwrap_or(Time(0)))?,
        tc(&args.range.1, seq.out_point().unwrap_or(seq.duration()))?,
    );
    if range.1 <= range.0 {
        return Err("the range is empty: out must come after in".into());
    }
    Ok(Request {
        path: args.output.clone(),
        range,
        preset,
        loudness: args.loudness,
        lossless: args.lossless,
    })
}

#[cfg(test)]
mod tests;
