//! DESIGN.md Layout > Checks on the real default layout, measured on rendered pixels
//! at 1280×720 and 1920×1080 (window size in points), 100% and 150% scale.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Color32, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use jvid_core::history::Command;
use suite_ui::test_support::gpu;

use super::*;

const SIZES: [[f32; 2]; 2] = [[1280.0, 720.0], [1920.0, 1080.0]];
const SCALES: [f32; 2] = [1.0, 1.5];

fn configs() -> impl Iterator<Item = ([f32; 2], f32)> {
    SIZES
        .into_iter()
        .flat_map(|size| SCALES.map(|ppp| (size, ppp)))
}

fn builder(size: [f32; 2], ppp: f32) -> egui_kittest::HarnessBuilder<App> {
    Harness::builder()
        .with_size(size)
        .with_pixels_per_point(ppp)
}

fn harness(size: [f32; 2], ppp: f32) -> Harness<'static, App> {
    let mut h = builder(size, ppp).wgpu().build_eframe(|cc| App::new(cc));
    h.run();
    h
}

/// A rendered frame plus the panels' rects, in physical pixels.
struct Shot {
    pixels: Vec<Color32>,
    width: u32,
    ppp: f32,
    panels: Vec<(Tab, Rect)>,
    ctx: String,
}

impl Shot {
    fn take(h: &mut Harness<'_, App>, ctx: String) -> Self {
        let ppp = h.ctx.pixels_per_point();
        let panels = h
            .state()
            .dock
            .iter_leaves()
            .map(|(_, leaf)| (leaf.tabs[0], leaf.rect))
            .collect();
        let img = h.render().expect("render");
        Self {
            pixels: img
                .pixels()
                .map(|p| Color32::from_rgb(p[0], p[1], p[2]))
                .collect(),
            width: img.width(),
            ppp,
            panels,
            ctx,
        }
    }

    fn px(&self, pt: f32) -> u32 {
        (pt * self.ppp).round() as u32
    }

    fn at(&self, x: u32, y: u32) -> Color32 {
        self.pixels[(y * self.width + x) as usize]
    }

    fn panel(&self, tab: Tab) -> Rect {
        self.panels.iter().find(|(t, _)| *t == tab).unwrap().1
    }

    /// Runs of pixels that are neither Ground nor Panel, as (start, len) in pixel coords.
    fn runs(&self, points: impl Iterator<Item = (u32, u32)>, vertical: bool) -> Vec<(u32, u32)> {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (x, y) in points {
            let c = self.at(x, y);
            if near(c, color::GROUND) || near(c, color::PANEL) {
                continue;
            }
            let i = if vertical { y } else { x };
            match runs.last_mut() {
                Some((start, len)) if *start + *len == i => *len += 1,
                _ => runs.push((i, 1)),
            }
        }
        runs
    }

    /// The x of the panel's left border, on its header's centre row.
    fn left_border(&self, r: Rect) -> u32 {
        let y = self.px(r.top() + size::HEADER / 2.0);
        let x = self.px(r.left()) - 2;
        (x..).find(|&x| near(self.at(x, y), color::BORDER)).unwrap()
    }

    /// Top border, header divider and bottom border, down a column near the panel's
    /// right edge, clear of the rounded corners.
    fn column(&self, r: Rect) -> Vec<(u32, u32)> {
        let x = self.px(r.right() - 12.0);
        self.runs(
            (self.px(r.top())..self.px(r.bottom())).map(|y| (x, y)),
            true,
        )
    }

    /// Text pixels in the panel's header: (left x, top y, baseline y).
    fn label(&self, r: Rect) -> (u32, u32, u32) {
        let xs = self.px(r.left()) + 1..self.px(r.right()) - 1;
        let ys = self.px(r.top()) + 1..self.px(r.top() + size::HEADER) - 1;
        let mut rows = Vec::new();
        let mut left = u32::MAX;
        for y in ys {
            let n = xs.clone().filter(|&x| is_text(self.at(x, y))).count();
            if n > 0 {
                left = left.min(xs.clone().find(|&x| is_text(self.at(x, y))).unwrap());
            }
            rows.push((y, n));
        }
        let max = rows.iter().map(|r| r.1).max().unwrap();
        assert!(max > 0, "{}: no header text in {r:?}", self.ctx);
        let top = rows.iter().find(|r| r.1 > 0).unwrap().0;
        // Descenders (g, p) are a few pixels; the baseline row is dense.
        let baseline = rows.iter().rev().find(|r| r.1 * 3 >= max).unwrap().0;
        (left, top, baseline)
    }

    /// Panels whose tops line up, row by row.
    fn rows(&self) -> Vec<Vec<(Tab, Rect)>> {
        let mut rows: Vec<Vec<(Tab, Rect)>> = Vec::new();
        for &(t, r) in &self.panels {
            match rows.iter_mut().find(|row| row[0].1.top() == r.top()) {
                Some(row) => row.push((t, r)),
                None => rows.push(vec![(t, r)]),
            }
        }
        rows.sort_by(|a, b| a[0].1.top().total_cmp(&b[0].1.top()));
        rows
    }
}

fn near(a: Color32, b: Color32) -> bool {
    a.r().abs_diff(b.r()) <= 2 && a.g().abs_diff(b.g()) <= 2 && a.b().abs_diff(b.b()) <= 2
}

/// Bright enough to be a label: Ink 3 and up, not Panel or Raised.
fn is_text(c: Color32) -> bool {
    c.r().max(c.g()).max(c.b()) > 100
}

fn shots() -> impl Iterator<Item = Shot> {
    configs().map(|(size, ppp)| {
        let mut h = harness(size, ppp);
        Shot::take(&mut h, format!("{}x{} @{ppp}x", size[0], size[1]))
    })
}

#[test]
fn default_layout_has_two_rows_of_panels() {
    if !gpu() {
        return;
    }
    for s in shots() {
        let rows = s.rows();
        let tabs: Vec<Vec<Tab>> = rows
            .iter()
            .map(|row| {
                let mut row = row.clone();
                row.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
                row.into_iter().map(|p| p.0).collect()
            })
            .collect();
        use Tab::*;
        assert_eq!(
            tabs,
            vec![vec![Media, Preview, Inspector], vec![Timeline]],
            "{}",
            s.ctx
        );
        for (_, r) in &s.panels {
            assert!(r.width() >= size::PANEL_MIN, "{}: {r:?}", s.ctx);
        }
    }
}

#[test]
fn header_lines_and_baselines_run_straight() {
    if !gpu() {
        return;
    }
    for s in shots() {
        for row in s.rows() {
            let lines: Vec<_> = row
                .iter()
                .map(|&(t, r)| (t, s.column(r)[1], s.label(r).2))
                .collect();
            let (_, divider, baseline) = lines[0];
            for &(t, d, b) in &lines {
                assert_eq!(d, divider, "{}: {t:?} divider vs {lines:?}", s.ctx);
                assert_eq!(b, baseline, "{}: {t:?} baseline vs {lines:?}", s.ctx);
            }
        }
    }
}

#[test]
fn hairlines_are_one_physical_pixel() {
    if !gpu() {
        return;
    }
    for s in shots() {
        for &(t, r) in &s.panels {
            let down = s.column(r);
            let ctx = format!("{}: {t:?} down {down:?}", s.ctx);
            assert_eq!(down.len(), 3, "{ctx}");
            assert!(down.iter().all(|r| r.1 == 1), "{ctx}");
            assert_eq!(down[1].0 - down[0].0, s.px(size::HEADER) - 1, "{ctx}");
        }
        for row in s.rows() {
            // Across the (empty) content of every panel in the row, gutters included.
            let y = s.px(row[0].1.bottom() - 20.0);
            let across = s.runs((0..s.width).map(|x| (x, y)), false);
            assert_eq!(across.len(), 2 * row.len(), "{}: {across:?}", s.ctx);
            assert!(across.iter().all(|r| r.1 == 1), "{}: {across:?}", s.ctx);
        }
    }
}

#[test]
fn titles_start_twelve_in() {
    if !gpu() {
        return;
    }
    for s in shots() {
        for (t, r) in s.panels.iter().filter(|(t, _)| *t != Tab::Preview) {
            let inset = s.left_border(*r) + s.px(space::INSET);
            let left = s.label(*r).0;
            // Inter's left side bearing is under 1pt.
            assert!(
                (inset..=inset + s.px(1.0)).contains(&left),
                "{}: {t:?} title starts at {left}, not {inset}",
                s.ctx
            );
        }
        // The tabbed preview: the tab group starts 4px in, so its active pill does too
        // once it is the first tab.
        let mut h = harness([1280.0, 720.0], s.ppp);
        h.state_mut().preview_active = 0;
        h.run();
        let s2 = Shot::take(&mut h, s.ctx.clone());
        let r = s2.panel(Tab::Preview);
        let y = s2.px(r.top() + size::HEADER / 2.0);
        let border = s2.left_border(r);
        let pill = (border..)
            .find(|&x| near(s2.at(x, y), color::RAISED))
            .unwrap();
        assert_eq!(pill - border, s2.px(space::S4), "{}", s2.ctx);
    }
}

#[test]
fn header_holds_still_between_one_view_and_tabs() {
    if !gpu() {
        return;
    }
    for (size, ppp) in configs() {
        let mut h = harness(size, ppp);
        let ctx = format!("{}x{} @{ppp}x", size[0], size[1]);
        let tabs = Shot::take(&mut h, ctx.clone());
        h.state_mut().preview_views = &["Program"];
        h.run();
        let one = Shot::take(&mut h, ctx.clone());
        let (rt, ro) = (tabs.panel(Tab::Preview), one.panel(Tab::Preview));
        assert_eq!(rt, ro, "{ctx}: panel moved");
        assert_eq!(tabs.column(rt), one.column(ro), "{ctx}: header lines moved");
        let (lt, lo) = (tabs.label(rt), one.label(ro));
        assert_eq!(lt.2, lo.2, "{ctx}: baseline tabs {lt:?} vs one view {lo:?}");
        assert!(lt.1.abs_diff(lo.1) <= 1, "{ctx}: top {lt:?} vs {lo:?}");
    }
}

#[test]
fn snapshots() {
    if !gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    for (size, ppp) in configs() {
        let mut h = harness(size, ppp);
        results.add(h.try_snapshot(format!("editor_{}x{}_{ppp}x", size[0], size[1])));
    }
}

fn enabled(h: &Harness<'_, App>, label: &str) -> bool {
    !h.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn undo_redo_and_dirty_marker_follow_history() {
    let mut h = builder([1280.0, 720.0], 1.0).build_eframe(|cc| App::new(cc));
    h.run();
    assert!(!enabled(&h, "Undo") && !enabled(&h, "Redo"));
    assert_eq!(h.state().window_title, "Untitled — jvid");

    let asset = Command::AddAsset(PathBuf::from("a.mkv"));
    h.state_mut().history.apply(asset).unwrap();
    h.run();
    assert!(enabled(&h, "Undo") && !enabled(&h, "Redo"));
    assert_eq!(h.state().window_title, "Untitled • — jvid");

    h.get_by_label("Undo").click();
    h.run();
    assert!(!enabled(&h, "Undo") && enabled(&h, "Redo"));
    assert_eq!(h.state().window_title, "Untitled — jvid");

    h.get_by_label("Redo").click();
    h.run();
    assert!(enabled(&h, "Undo") && !enabled(&h, "Redo"));
}

#[derive(Default)]
struct Memory(HashMap<String, String>);

impl eframe::Storage for Memory {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

#[test]
fn dock_layout_persists() {
    let mut app = App::restore(None);
    // Close the inspector, as a rearranged layout.
    let inspector = app.dock.find_tab(&Tab::Inspector).unwrap();
    app.dock.remove_tab(inspector);
    let mut storage = Memory::default();
    eframe::App::save(&mut app, &mut storage);

    let restored = App::restore(Some(&storage));
    assert!(restored.dock.find_tab(&Tab::Inspector).is_none());
    assert!(restored.dock.find_tab(&Tab::Timeline).is_some());
    for (_, node) in restored.dock.iter_all_nodes() {
        assert!(!node.is_leaf() || node.is_tab_bar_hidden());
    }
    assert!(App::restore(None).dock.find_tab(&Tab::Inspector).is_some());
}
