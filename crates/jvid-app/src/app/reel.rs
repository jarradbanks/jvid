//! The README's preview reel: the real editor driven frame by frame through egui_kittest and
//! wgpu, one PNG per frame at 30 fps, with a drawn cursor. `scripts/preview/build` cuts the
//! footage, runs this and encodes the frames; run that rather than this test directly.
//!
//! Playback is deterministic: a fake audio device owns the master clock and is pulled exactly
//! one frame's worth of samples per reel frame, and each frame waits for its decode.

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use image::{Rgba, RgbaImage};
use jvid_audio::{Backend, Feed, Output, SAMPLE_RATE};
use jvid_core::history::Command;
use jvid_core::time::FLICKS_PER_SECOND;
use jvid_jobs::Priority;

use super::*;

const FPS: u32 = 30;
const SIZE: [f32; 2] = [1600.0, 900.0];

/// A device that plays only when the reel pulls from it.
#[derive(Clone, Default)]
struct Device(Arc<Mutex<Option<Arc<Feed>>>>);

impl Backend for Device {
    fn devices(&self) -> Vec<String> {
        vec!["reel".into()]
    }
    fn open(
        &mut self,
        _: Option<&str>,
        feed: Arc<Feed>,
    ) -> Result<(String, Box<dyn std::any::Any>), String> {
        *self.0.lock().unwrap() = Some(feed);
        Ok(("reel".into(), Box::new(())))
    }
}

struct Reel {
    h: Harness<'static, App>,
    device: Device,
    dir: PathBuf,
    frame: u32,
    cursor: Pos2,
}

impl Reel {
    /// Renders and saves one frame: advances the clock a frame's worth, runs the UI with the
    /// queued input, then steps until the Program shows the exact frame and background jobs
    /// are done, so the result doesn't depend on how fast this machine decodes.
    fn frame(&mut self) {
        if self.h.state().preview.program_playing() {
            let samples = (SAMPLE_RATE / FPS) as usize;
            let mut buf = vec![0.0; samples * 2];
            if let Some(feed) = self.device.0.lock().unwrap().as_ref() {
                feed.fill(&mut buf, 2);
            }
        }
        self.h.input_mut().time = Some(self.frame as f64 / FPS as f64);
        self.h.step();
        let deadline = Instant::now() + Duration::from_secs(60);
        while !self.settled() {
            assert!(
                Instant::now() < deadline,
                "frame {} never settled",
                self.frame
            );
            std::thread::sleep(Duration::from_millis(2));
            self.h.step();
        }
        let mut img = self.screenshot();
        cursor(&mut img, self.cursor);
        img.save(self.dir.join(format!("{:05}.png", self.frame)))
            .expect("save frame");
        self.frame += 1;
    }

    /// The frame without kittest's own cursor: a screenshot request, filled after one more
    /// step and taken back out of the input before the app sees it.
    fn screenshot(&mut self) -> RgbaImage {
        self.h
            .ctx
            .send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        self.h.step();
        let events = &mut self.h.input_mut().events;
        let i = events
            .iter()
            .position(|e| matches!(e, egui::Event::Screenshot { .. }))
            .expect("screenshot");
        let egui::Event::Screenshot { image, .. } = events.remove(i) else {
            unreachable!()
        };
        let [w, h] = image.size;
        let bytes = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        RgbaImage::from_raw(w as u32, h as u32, bytes).unwrap()
    }

    fn settled(&self) -> bool {
        let app = self.h.state();
        let empty = app.history.project().sequence().duration() == jvid_core::time::Time(0);
        (empty || app.preview.program_caught_up()) && app.timeline.jobs_idle()
    }

    fn hold(&mut self, secs: f32) {
        for _ in 0..frames(secs) {
            self.frame();
        }
    }

    /// Moves the cursor to `to` over `secs`, easing in and out.
    fn move_to(&mut self, to: Pos2, secs: f32) {
        let from = self.cursor;
        let n = frames(secs);
        for i in 1..=n {
            let t = ease(i as f32 / n as f32);
            self.cursor = from + (to - from) * t;
            self.h.hover_at(self.cursor);
            self.frame();
        }
    }

    fn press(&mut self) {
        self.h.drag_at(self.cursor);
        self.frame();
    }

    fn release(&mut self) {
        self.h.drop_at(self.cursor);
        self.frame();
    }

    fn click(&mut self) {
        self.press();
        self.release();
    }

    fn rect(&self, label: &str) -> Rect {
        self.h.get_by_label(label).rect()
    }
}

fn frames(secs: f32) -> u32 {
    (secs * FPS as f32).round() as u32
}

/// Cubic ease in and out.
fn ease(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// A pointer arrow, tip at the origin, in points: its outline, then its fill.
const OUTLINE: [[f32; 2]; 7] = [
    [0.0, 0.0],
    [0.0, 17.5],
    [4.3, 13.6],
    [7.2, 20.0],
    [10.2, 18.7],
    [7.4, 12.5],
    [13.0, 12.5],
];
const FILL: [[f32; 2]; 7] = [
    [1.3, 3.1],
    [1.3, 14.4],
    [4.8, 11.3],
    [7.8, 18.1],
    [8.5, 17.8],
    [5.5, 11.2],
    [9.9, 11.2],
];

/// Draws the arrow at `at`, antialiased with 4x4 samples, over a faint drop shadow.
fn cursor(img: &mut RgbaImage, at: Pos2) {
    let scale = 1.15;
    let poly = |p: &[[f32; 2]], off: f32| -> Vec<Pos2> {
        p.iter()
            .map(|q| at + vec2(q[0], q[1] + off) * scale)
            .collect()
    };
    paint(img, &poly(&OUTLINE, 1.2), [0, 0, 0], 0.22);
    paint(img, &poly(&OUTLINE, 0.0), [12, 12, 14], 1.0);
    paint(img, &poly(&FILL, 0.0), [250, 250, 250], 1.0);
}

fn paint(img: &mut RgbaImage, poly: &[Pos2], rgb: [u8; 3], alpha: f32) {
    let r = Rect::from_points(poly);
    for y in r.top().floor() as i32..=r.bottom().ceil() as i32 {
        for x in r.left().floor() as i32..=r.right().ceil() as i32 {
            if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
                continue;
            }
            let hits = (0..16)
                .filter(|i| {
                    let p = pos2(
                        x as f32 + (i % 4) as f32 / 4.0 + 0.125,
                        y as f32 + (i / 4) as f32 / 4.0 + 0.125,
                    );
                    inside(poly, p)
                })
                .count();
            if hits == 0 {
                continue;
            }
            let a = alpha * hits as f32 / 16.0;
            let px = img.get_pixel_mut(x as u32, y as u32);
            for c in 0..3 {
                px[c] = (px[c] as f32 * (1.0 - a) + rgb[c] as f32 * a).round() as u8;
            }
            *px = Rgba([px[0], px[1], px[2], 255]);
        }
    }
}

/// Even-odd point in polygon.
fn inside(poly: &[Pos2], p: Pos2) -> bool {
    let mut odd = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            odd = !odd;
        }
        j = i;
    }
    odd
}

/// Imports the shots and puts them end to end, each with half a second of handle at either
/// end so a crossfade has media past the cut, then fits the timeline to them.
fn project(h: &mut Harness<'static, App>, media: &Path) {
    jvid_media::ffmpeg::init().unwrap();
    let names = ["bridge", "walk", "face", "tower"];
    let items = names
        .iter()
        .map(|n| {
            let path = media.join(format!("{n}.mp4"));
            let probe = jvid_media::probe::probe(&path).unwrap();
            (path, probe)
        })
        .collect();
    let app = h.state_mut();
    app.history.apply(Command::Import(items)).unwrap();
    let assets: Vec<AssetId> = app
        .history
        .project()
        .assets()
        .iter()
        .map(|a| a.id)
        .collect();
    let handle = FLICKS_PER_SECOND / 2;
    let mut at = jvid_core::time::Time(0);
    for &asset in &assets {
        let mut p = app
            .history
            .project()
            .placement(asset, at, false, None, false)
            .unwrap();
        p.source_in.0 += handle;
        p.source_out.0 -= handle;
        at = p.start + (p.source_out - p.source_in);
        app.history.apply(Command::Place(p)).unwrap();
    }
    h.run_steps(2);
    let app = h.state_mut();
    assert!(app.timeline.action("zoom_fit", app.history.project()));
    app.history.mark_saved();
    app.path = Some(media.with_file_name("Tears of Steel.jvid"));
    app.bin.selected = None;
}

#[test]
#[ignore = "renders the README preview; run scripts/preview/build"]
fn reel() {
    let (Some(media), Some(dir)) = (
        std::env::var_os("JVID_REEL_MEDIA"),
        std::env::var_os("JVID_REEL_FRAMES"),
    ) else {
        panic!("run scripts/preview/build");
    };
    assert!(suite_ui::test_support::gpu(), "the reel needs a GPU");
    suite_ui::i18n::set_language("en");
    // A fresh effects folder holding the bundled examples, as tests have no real one.
    let effects = Path::new(&media).with_file_name("effects");
    let _ = std::fs::remove_dir_all(&effects);
    crate::effects::scan_for_test(&effects);
    let mut h = Harness::builder()
        .with_size(SIZE)
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / FPS as f32)
        .wgpu()
        .build_eframe(|cc| App::new(cc, &Default::default()));
    h.state_mut().on_welcome = false;
    h.run();
    project(&mut h, Path::new(&media));
    let device = Device::default();
    h.state_mut()
        .preview
        .set_output(Output::new(Box::new(device.clone()), None));
    let mut r = Reel {
        h,
        device,
        dir: dir.into(),
        frame: 0,
        cursor: start(),
    };
    r.edit();
}

impl Reel {
    /// The lanes' left edge and V1's lane centre, the lowest of the Hide toggles.
    fn lane(&self) -> (f32, f32) {
        let app = self.h.state();
        let (_, leaf) = app
            .dock
            .iter_leaves()
            .find(|(_, l)| l.tabs.contains(&Tab::Timeline))
            .unwrap();
        let left = leaf.rect.left() + space::S12 + size::TRACK_HEADER;
        let y = self
            .h
            .get_all_by_label("Hide")
            .map(|h| h.rect().center().y)
            .fold(f32::MIN, f32::max);
        (left, y)
    }

    /// The ruler's centre line: just above the top track.
    fn ruler(&self) -> f32 {
        let top = self
            .h
            .get_all_by_label("Hide")
            .map(|h| h.rect().center().y)
            .fold(f32::MAX, f32::min);
        top - size::TRACK / 2.0 - 12.0
    }

    fn x_at(&self, secs: f64) -> f32 {
        let (left, _) = self.lane();
        let t = jvid_core::time::Time((secs * FLICKS_PER_SECOND as f64) as i64);
        self.h.state().timeline.x_at(t, left)
    }

    fn duration(&self) -> f64 {
        secs(self.h.state().history.project().sequence().duration())
    }

    fn playhead(&self) -> f64 {
        secs(self.h.state().timeline.playhead())
    }

    /// Where the walk shot starts on V1: the last clip starting by 2.6s, cut or not.
    fn walk(&self) -> f64 {
        let seq = self.h.state().history.project().sequence();
        seq.tracks()
            .iter()
            .find(|t| t.kind == jvid_core::model::TrackKind::Video)
            .unwrap()
            .clips()
            .iter()
            .map(|c| secs(c.start))
            .filter(|&s| s <= 2.6)
            .fold(0.0, f64::max)
    }

    /// Notes where a beat starts, for laying out scripts/preview/motion.html.
    fn mark(&self, beat: &str) {
        println!("reel: {beat} at {:.2}s", self.frame as f32 / FPS as f32);
    }

    fn key(&mut self, modifiers: egui::Modifiers, key: egui::Key) {
        self.h.key_press_modifiers(modifiers, key);
        self.frame();
    }

    /// Clicks the ruler at `secs`, moving the playhead there.
    fn seek(&mut self, secs: f64, travel: f32) {
        self.move_to(pos2(self.x_at(secs), self.ruler()), travel);
        self.click();
        let at = self.playhead();
        assert!((at - secs).abs() < 0.05, "seek to {secs} landed at {at}");
    }

    /// The storyboard: each beat is a short moment the composition cuts to.
    fn edit(&mut self) {
        self.hold(0.5);
        self.cut();
        self.grade();
        self.keys();
        self.dissolve();
        self.title();
        self.looks();
        self.export();
    }

    /// Takes a beat out of the bridge shot: split twice and ripple delete the piece between.
    fn cut(&mut self) {
        self.mark("cut");
        let before = self.duration();
        self.seek(1.0, 0.6);
        self.key(egui::Modifiers::COMMAND, egui::Key::K);
        self.hold(0.2);
        self.seek(1.6, 0.4);
        self.key(egui::Modifiers::COMMAND, egui::Key::K);
        self.hold(0.2);
        let (_, v1) = self.lane();
        self.move_to(pos2(self.x_at(1.3), v1), 0.4);
        self.click();
        self.hold(0.2);
        self.key(egui::Modifiers::SHIFT, egui::Key::Delete);
        let after = self.duration();
        assert!(
            (before - after - 0.6).abs() < 0.05,
            "cut {before} to {after}"
        );
        self.hold(0.6);
    }

    /// Selects the clip under the playhead, warms it up and adds a vignette from the Effects tab.
    fn grade(&mut self) {
        self.mark("grade");
        let (_, v1) = self.lane();
        self.move_to(pos2(self.x_at(self.playhead() - 0.2), v1), 0.5);
        self.click();
        self.hold(0.2);
        self.slide("Temperature", 0.5, 0.85, 0.7);
        self.slide("Saturation", 0.5, 0.7, 0.5);

        // The bin's tab, left of the Inspector's Effects section.
        let tab = self
            .h
            .get_all_by_label("Effects")
            .map(|n| n.rect())
            .min_by(|a, b| a.left().total_cmp(&b.left()))
            .unwrap();
        self.move_to(tab.center(), 0.6);
        self.click();
        self.hold(0.2);
        let row = self.rect("Vignette").center();
        self.move_to(row, 0.4);
        self.hold(0.1);
        self.press();
        self.move_to(pos2(self.x_at(self.playhead() - 0.2), v1), 0.7);
        self.release();
        self.hold(0.2);
        self.move_to(self.rect("Effects 1").center(), 0.5);
        self.click();
        self.hold(0.2);
        self.slide("Strength", 0.5, 1.0, 0.6);
        self.hold(0.5);
    }

    /// Drags the slider labelled `label` from `from` to `to` of its width.
    fn slide(&mut self, label: &str, from: f32, to: f32, secs: f32) {
        let r = self
            .h
            .get_by_role_and_label(egui::accesskit::Role::Slider, label)
            .rect();
        let at = |f: f32| pos2(r.lerp_inside(vec2(f, 0.5)).x, r.center().y);
        self.move_to(at(from), 0.5);
        self.hold(0.1);
        self.press();
        self.move_to(at(to), secs);
        self.release();
        self.hold(0.2);
    }

    /// Keys the walk shot's scale at its start and again larger, then plays the push-in.
    fn keys(&mut self) {
        self.mark("keys");
        let (_, v1) = self.lane();
        let walk = self.walk();
        self.seek(walk + 0.1, 0.6);
        self.move_to(pos2(self.x_at(walk + 0.6), v1), 0.4);
        self.click();
        self.hold(0.2);
        if self.h.query_by_label("Animate Scale").is_none() {
            // The grade left the Inspector on the clip's effects.
            self.move_to(self.rect("Video").center(), 0.5);
            self.click();
            self.hold(0.2);
        }
        let diamond = self.h.get_by_label("Animate Scale").rect().center();
        self.move_to(diamond, 0.6);
        self.click();
        self.hold(0.3);
        self.seek(walk + 1.6, 0.6);
        // 100% sits a quarter of the way along the Scale slider.
        self.slide("Scale", 0.25, 0.4, 0.6);
        self.seek(walk + 0.1, 0.6);
        self.move_to(start(), 0.4);
        self.key(egui::Modifiers::NONE, egui::Key::Space);
        self.hold(1.8);
        self.key(egui::Modifiers::NONE, egui::Key::Space);
        self.hold(0.3);
    }

    /// A crossfade on the walk shot's end, played across the cut.
    fn dissolve(&mut self) {
        self.mark("dissolve");
        self.key(egui::Modifiers::COMMAND, egui::Key::D);
        self.hold(0.3);
        let cut = self.walk() + 2.5;
        self.seek(cut - 0.9, 0.6);
        self.move_to(start(), 0.4);
        self.key(egui::Modifiers::NONE, egui::Key::Space);
        self.hold(1.6);
        self.key(egui::Modifiers::NONE, egui::Key::Space);
        self.hold(0.3);
    }

    /// A title from the Text tab onto a new track over the playhead, retitled in the Inspector.
    fn title(&mut self) {
        self.mark("title");
        self.move_to(self.rect("Text").center(), 0.6);
        self.click();
        self.hold(0.2);
        let preset = self.rect("Title").center();
        self.move_to(preset, 0.5);
        self.hold(0.1);
        // Onto V1's top edge, which opens a new track above it; starting before the playhead,
        // so the Program shows it.
        let (_, v1) = self.lane();
        let head = self.x_at(self.playhead());
        self.press();
        self.move_to(pos2(head - 60.0, v1 - size::TRACK / 2.0 + 8.0), 0.9);
        self.hold(0.3);
        self.release();
        self.hold(0.3);
        // Select it on the new top track, and show the Inspector's Text tab, the rightmost "Text".
        let v2 = self
            .h
            .get_all_by_label("Hide")
            .map(|h| h.rect().center().y)
            .fold(f32::MAX, f32::min);
        self.move_to(pos2(head + 40.0, v2), 0.5);
        self.click();
        self.hold(0.2);
        let tab = self
            .h
            .get_all_by_label("Text")
            .map(|n| n.rect())
            .max_by(|a, b| a.left().total_cmp(&b.left()))
            .unwrap();
        self.move_to(tab.center(), 0.5);
        self.click();
        self.hold(0.2);

        // Retitle it: double-click the word in the Inspector's text box and type.
        let text = self
            .h
            .get_by_role(egui::accesskit::Role::MultilineTextInput)
            .rect();
        let word = text.left_top() + vec2(20.0, 12.0);
        self.move_to(word, 0.7);
        self.click();
        self.click();
        self.hold(0.2);
        for c in "Tears of Steel".chars() {
            self.h.event(egui::Event::Text(c.into()));
            self.hold(0.06);
        }
        self.hold(0.5);
    }

    /// The built-in themes, then a run through some of the languages.
    fn looks(&mut self) {
        self.mark("themes");
        self.move_to(start(), 0.6);
        for theme in ["light", "high-contrast", "dark"] {
            self.h.state_mut().theme = suite_ui::themes::LiveTheme::new(theme);
            self.hold(1.0);
        }
        self.mark("languages");
        for lang in ["ja", "de", "es", "ko", "fr", "zh-Hans", "tr", "sv", "en"] {
            suite_ui::i18n::set_language(lang);
            self.hold(0.4);
        }
    }

    /// Export. The real job waits behind a blocked pool and is swapped for a stand-in whose
    /// progress the reel sets, so the header counts up the same on every machine.
    fn export(&mut self) {
        self.mark("export");
        let (unblock, blocked) = channel::<()>();
        let app = self.h.state_mut();
        app.exports = jvid_jobs::Pool::new(1);
        app.exports
            .submit("reel:block", Priority::Interactive, move |_| {
                let _ = blocked.recv();
            });
        app.export_dialog
            .set_path(PathBuf::from("~/Videos/Tears of Steel.mp4"));
        self.move_to(self.rect("Export").center(), 0.8);
        self.click();
        self.hold(0.7);
        let export = self
            .h
            .get_all_by_role_and_label(egui::accesskit::Role::Button, "Export")
            .map(|n| n.rect())
            .max_by(|a, b| a.top().total_cmp(&b.top()))
            .unwrap();
        self.move_to(export.center(), 0.7);
        self.hold(0.1);
        self.click();

        let (progress, rx) = channel::<f32>();
        let (ack, acked) = channel();
        let pool = jvid_jobs::Pool::new(1);
        let handle = pool.submit("reel:export", Priority::Interactive, move |ctx| {
            for p in rx {
                ctx.set_progress(p);
                let _ = ack.send(());
            }
        });
        // Held open so the stand-in never reads as crashed.
        let (_result, result) = channel();
        let app = self.h.state_mut();
        let row = app.queue.rows().remove(0);
        app.queue.cancel_all();
        app.queue = crate::export::queue::Queue::default();
        app.queue
            .push(row.path, row.preset, row.duration, (handle, result));
        let (from, home) = (self.cursor, start());
        let (n, m) = (frames(2.0), frames(1.2));
        for i in 1..=n {
            progress.send(i as f32 / n as f32).unwrap();
            acked.recv().unwrap();
            self.cursor = from + (home - from) * ease((i as f32 / m as f32).min(1.0));
            self.h.hover_at(self.cursor);
            self.frame();
        }
        self.hold(1.0);
        drop(progress);
        drop(unblock);
    }
}

/// Where the cursor rests at the start and the end.
fn start() -> Pos2 {
    pos2(SIZE[0] * 0.55, SIZE[1] * 0.62)
}

fn secs(t: jvid_core::time::Time) -> f64 {
    t.0 as f64 / FLICKS_PER_SECOND as f64
}
