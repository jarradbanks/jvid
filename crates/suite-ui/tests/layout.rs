//! DESIGN.md Layout > Checks for the panel frame, 32px header and tabs, measured on
//! rendered pixels at 100% and 150% scale. Headless wgpu on any Vulkan driver;
//! `suite_ui::test_support::gpu()` picks Mesa lavapipe when installed (see CONTRIBUTING.md).

use suite_ui::test_support as common;

use egui::Color32;
use egui_kittest::Harness;
use suite_ui::panel::Panel;
use suite_ui::tokens::{color, size, space};

const SCALES: [f32; 2] = [1.0, 1.5];
/// The panel's top-left corner and size, in points.
const ORIGIN: f32 = 4.0;
const W: f32 = 320.0;
const H: f32 = 120.0;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Title,
    Tabs,
}

fn harness(ppp: f32, mode: Mode, focused: bool) -> Harness<'static> {
    let mut active = 0;
    let mut fresh = true;
    Harness::builder()
        .with_size([W + 2.0 * ORIGIN, H + 2.0 * ORIGIN])
        .with_pixels_per_point(ppp)
        .wgpu()
        .build_ui(move |ui| {
            if common::install_theme(ui, &mut fresh) {
                return;
            }
            ui.painter()
                .rect_filled(ui.ctx().content_rect(), 0.0, color::ground());
            let rect = egui::Rect::from_min_size(egui::pos2(ORIGIN, ORIGIN), egui::vec2(W, H));
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                let panel = match mode {
                    Mode::Title => Panel::titled("Media"),
                    Mode::Tabs => Panel::tabbed(&["Media", "Effects"], &mut active),
                };
                panel.focused(focused).show(ui, |ui| {
                    ui.label("Media");
                });
            });
        })
}

struct Shot {
    pixels: Vec<Color32>,
    width: u32,
    height: u32,
    ppp: f32,
}

impl Shot {
    fn take(ppp: f32, mode: Mode, focused: bool) -> Self {
        let mut h = harness(ppp, mode, focused);
        h.run();
        let img = h.render().expect("render");
        Self {
            pixels: img
                .pixels()
                .map(|p| Color32::from_rgb(p[0], p[1], p[2]))
                .collect(),
            width: img.width(),
            height: img.height(),
            ppp,
        }
    }

    fn px(&self, pt: f32) -> u32 {
        (pt * self.ppp).round() as u32
    }

    fn at(&self, x: u32, y: u32) -> Color32 {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Runs of pixels that are neither Ground nor Panel along a line, as (start, len).
    fn line_runs(&self, points: impl Iterator<Item = (u32, u32)>) -> Vec<(u32, u32)> {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (i, (x, y)) in points.enumerate() {
            let c = self.at(x, y);
            if near(c, color::ground()) || near(c, color::panel()) {
                continue;
            }
            match runs.last_mut() {
                Some((start, len)) if *start + *len == i as u32 => *len += 1,
                _ => runs.push((i as u32, 1)),
            }
        }
        runs
    }

    /// Bounding box (x0, y0, x1, y1) of text-bright pixels inside a pixel region.
    fn ink_box(&self, x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> (u32, u32, u32, u32) {
        let mut b = (u32::MAX, u32::MAX, 0, 0);
        for py in y {
            for px in x.clone() {
                let c = self.at(px, py);
                if c.r().max(c.g()).max(c.b()) > 140 {
                    b = (b.0.min(px), b.1.min(py), b.2.max(px), b.3.max(py));
                }
            }
        }
        assert!(b.0 != u32::MAX, "no text found");
        b
    }

    fn header_rows(&self) -> std::ops::Range<u32> {
        self.px(ORIGIN) + 1..self.px(ORIGIN + size::HEADER) - 1
    }
}

fn near(a: Color32, b: Color32) -> bool {
    a.r().abs_diff(b.r()) <= 2 && a.g().abs_diff(b.g()) <= 2 && a.b().abs_diff(b.b()) <= 2
}

#[test]
fn hairlines_are_one_physical_pixel() {
    if !common::gpu() {
        return;
    }
    for ppp in SCALES {
        for mode in [Mode::Title, Mode::Tabs] {
            for focused in [false, true] {
                let s = Shot::take(ppp, mode, focused);
                let ctx = format!("{ppp}x {mode:?} focused={focused}");
                // Down the middle (between the two tabs): top border, header divider, bottom border.
                let x = s.px(ORIGIN + W / 2.0);
                let down = s.line_runs((0..s.height).map(|y| (x, y)));
                let top = s.px(ORIGIN);
                let divider = s.px(ORIGIN + size::HEADER) - 1;
                let bottom = s.px(ORIGIN + H) - 1;
                assert_eq!(down, vec![(top, 1), (divider, 1), (bottom, 1)], "{ctx}");
                // Across the content, below the label: left and right borders.
                let y = s.px(ORIGIN + H - 20.0);
                let across = s.line_runs((0..s.width).map(|x| (x, y)));
                let right = s.px(ORIGIN + W) - 1;
                assert_eq!(across, vec![(top, 1), (right, 1)], "{ctx}");
                let border = if focused {
                    color::blue()
                } else {
                    color::border()
                };
                assert!(near(s.at(x, top), border), "{ctx}");
                assert!(near(s.at(x, divider), color::border()), "{ctx}");
            }
        }
    }
}

#[test]
fn header_holds_still_between_title_and_tabs() {
    if !common::gpu() {
        return;
    }
    for ppp in SCALES {
        let title = Shot::take(ppp, Mode::Title, false);
        let tabs = Shot::take(ppp, Mode::Tabs, false);
        let left = title.px(ORIGIN) + 1..title.px(ORIGIN + 80.0);
        let t = title.ink_box(left.clone(), title.header_rows());
        let b = tabs.ink_box(left, tabs.header_rows());
        // "Media" has no descenders, so its last ink row is the baseline. The cap
        // height may differ by an antialiased row, as the tab label sits at another x.
        assert_eq!(t.3, b.3, "{ppp}x baseline: title {t:?} vs tabs {b:?}");
        assert!(
            t.1.abs_diff(b.1) <= 1,
            "{ppp}x top: title {t:?} vs tabs {b:?}"
        );
    }
}

#[test]
fn title_and_content_start_twelve_in() {
    if !common::gpu() {
        return;
    }
    for ppp in SCALES {
        let s = Shot::take(ppp, Mode::Title, false);
        let inset = s.px(ORIGIN + space::INSET);
        let x = s.px(ORIGIN) + 1..s.px(ORIGIN + W / 2.0);
        let header = s.ink_box(x.clone(), s.header_rows());
        let content = s.ink_box(x, s.px(ORIGIN + size::HEADER) + 1..s.px(ORIGIN + H) - 1);
        // Inter's left side bearing on "M" is under 1pt.
        for (what, b) in [("header", header), ("content", content)] {
            assert!(
                (inset..=inset + s.px(1.0)).contains(&b.0),
                "{ppp}x {what} text starts at {} not {inset}",
                b.0
            );
        }
    }
}

#[test]
fn tabs_start_four_in_with_centred_labels() {
    if !common::gpu() {
        return;
    }
    for ppp in SCALES {
        let s = Shot::take(ppp, Mode::Tabs, false);
        let y = s.px(ORIGIN + size::HEADER / 2.0);
        // The active pill is Raised; find where it starts and ends on the header's centre row.
        let raised: Vec<u32> = (0..s.width)
            .filter(|&x| near(s.at(x, y), color::raised()))
            .collect();
        let (x0, x1) = (raised[0], *raised.last().unwrap());
        assert_eq!(x0, s.px(ORIGIN + space::S4), "{ppp}x pill start");
        let pill_rows = s.px(ORIGIN + 4.0) + 1..s.px(ORIGIN + 28.0) - 1;
        let label = s.ink_box(x0..x1 + 1, pill_rows);
        let (pill_mid, label_mid) = (x0 + x1, label.0 + label.2);
        assert!(
            pill_mid.abs_diff(label_mid) <= 2 * s.px(1.0),
            "{ppp}x label not centred: pill {x0}..{x1}, label {}..{}",
            label.0,
            label.2
        );
    }
}

#[test]
fn snapshots() {
    if !common::gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    for ppp in SCALES {
        for (mode, focused) in [(Mode::Title, false), (Mode::Tabs, true)] {
            let mut h = harness(ppp, mode, focused);
            h.run();
            results.add(h.try_snapshot(format!("panel_{mode:?}_{ppp}x").to_lowercase()));
        }
    }
}

#[test]
fn popup_border_is_one_physical_pixel() {
    if !common::gpu() {
        return;
    }
    for ppp in SCALES {
        let mut fresh = true;
        let mut h = Harness::builder()
            .with_size([120.0, 80.0])
            .with_pixels_per_point(ppp)
            .wgpu()
            .build_ui(move |ui| {
                if common::install_theme(ui, &mut fresh) {
                    return;
                }
                ui.painter()
                    .rect_filled(ui.ctx().content_rect(), 0.0, color::ground());
                // The frame egui's dropdown and chevron menus use.
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_size(egui::vec2(80.0, 40.0));
                });
            });
        h.run();
        let img = h.render().expect("render");
        let y = (30.0 * ppp) as u32;
        // Left to right: Ground, the border, then Raised fill. Anything else is a blended second row.
        let border: Vec<u32> = (0..img.width() / 2)
            .filter(|&x| {
                let p = img.get_pixel(x, y);
                let c = Color32::from_rgb(p[0], p[1], p[2]);
                !near(c, color::ground()) && !near(c, color::raised())
            })
            .collect();
        assert_eq!(border.len(), 1, "{ppp}x popup border columns {border:?}");
    }
}

#[test]
fn a_translucent_swatch_shows_the_checkerboard_and_an_opaque_one_is_solid() {
    use egui_kittest::kittest::Queryable;
    use suite_ui::widgets::ColorField;
    if !common::gpu() {
        return;
    }
    // The swatch's middle row, 2pt in from each side of the 24pt swatch.
    let row = |rgba: [u8; 4]| {
        let mut fresh = true;
        let mut h = Harness::builder()
            .with_size([200.0, 40.0])
            .wgpu()
            .build_ui(move |ui| {
                if common::install_theme(ui, &mut fresh) {
                    return;
                }
                let mut c = rgba;
                ui.add(ColorField::new(&mut c, "Box Color"));
            });
        h.run();
        let rect = h.get_by_role(egui::accesskit::Role::ColorWell).rect();
        let img = h.render().expect("render");
        let y = (rect.top() + size::CONTROL / 2.0) as u32;
        let x = rect.left() as u32;
        (x + 2..x + 22)
            .map(|x| img.get_pixel(x, y).0[0])
            .collect::<Vec<_>>()
    };
    let solid = row([0, 0, 0, 255]);
    assert!(solid.iter().all(|v| *v == 0), "{solid:?}");
    let half = row([0, 0, 0, 128]);
    // A dark cell between light ones, under 50% black.
    let dark = half.iter().filter(|v| (66..=72).contains(*v)).count();
    let light = half.iter().filter(|v| (91..=97).contains(*v)).count();
    assert!(dark >= 6 && light >= 8, "{half:?}");
}
