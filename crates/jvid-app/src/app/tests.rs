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
use crate::import::Importer;

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
    let mut h = welcome_harness(size, ppp);
    h.state_mut().on_welcome = false;
    h.run();
    h
}

fn welcome_harness(size: [f32; 2], ppp: f32) -> Harness<'static, App> {
    let mut h = builder(size, ppp)
        .wgpu()
        .build_eframe(|cc| App::new(cc, &Default::default()));
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
            // The timeline isn't empty: its lanes are Field.
            if row.iter().any(|(t, _)| *t == Tab::Timeline) {
                continue;
            }
            // Across the (empty) content of every panel in the row, gutters included,
            // above the media bin's pinned Import button.
            let y = s.px(row[0].1.bottom() - 56.0);
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
        let titled = |t: &Tab| ![Tab::Preview, Tab::Media].contains(t);
        for (t, r) in s.panels.iter().filter(|(t, _)| titled(t)) {
            let inset = s.left_border(*r) + s.px(space::INSET);
            let left = s.label(*r).0;
            // Inter's left side bearing is under 1pt.
            assert!(
                (inset..=inset + s.px(1.0)).contains(&left),
                "{}: {t:?} title starts at {left}, not {inset}",
                s.ctx
            );
        }
        // The tabbed panels: the tab group starts 4px in, so the active pill does too
        // once it is the first tab.
        let mut h = harness([1280.0, 720.0], s.ppp);
        h.state_mut().preview_active = 0;
        h.run();
        let s2 = Shot::take(&mut h, s.ctx.clone());
        for tab in [Tab::Preview, Tab::Media] {
            let r = s2.panel(tab);
            let y = s2.px(r.top() + size::HEADER / 2.0);
            let border = s2.left_border(r);
            let pill = (border..)
                .find(|&x| near(s2.at(x, y), color::RAISED))
                .unwrap();
            assert_eq!(pill - border, s2.px(space::S4), "{}: {tab:?}", s2.ctx);
        }
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

/// A bin of five assets, the second used by two clips.
fn bin_harness() -> Harness<'static, App> {
    use jvid_core::model::{AssetId, NewClip, ProbeInfo, TrackKind};
    use jvid_core::time::{FLICKS_PER_SECOND, Time};
    let mut h = harness([1280.0, 720.0], 1.0);
    let history = &mut h.state_mut().history;
    let video = |secs: i64| ProbeInfo {
        duration: Time(secs * FLICKS_PER_SECOND),
        width: Some(1920),
        height: Some(1080),
        ..Default::default()
    };
    let audio = ProbeInfo {
        duration: Time(185 * FLICKS_PER_SECOND),
        audio_streams: 1,
        ..Default::default()
    };
    let items = vec![
        ("/m/arena_01.mkv".into(), video(7471)),
        ("/m/clutch_round_3.mkv".into(), video(47)),
        ("/m/push.mp4".into(), video(729)),
        ("/m/music.flac".into(), audio),
        ("/m/finale.mkv".into(), video(93)),
    ];
    history.apply(Command::Import(items)).unwrap();
    let Ok(jvid_core::history::Created::Track(track)) = history.apply(Command::AddTrack {
        kind: TrackKind::Video,
        name: "V1".into(),
    }) else {
        panic!()
    };
    for start in [0, 100] {
        let clip = NewClip {
            asset: AssetId(2),
            source_in: Time(0),
            source_out: Time(50),
            start: Time(start),
            link: None,
        };
        history.apply(Command::AddClip { track, clip }).unwrap();
    }
    // The program's decodes of the (missing) clip are async; wait until they've failed.
    for _ in 0..500 {
        h.run();
        if h.state().preview.settled() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().preview.settled(), "preview decode never finished");
    h
}

#[test]
fn bin_snapshots() {
    if !gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    let mut h = bin_harness();
    h.state_mut().bin.selected = Some(jvid_core::model::AssetId(2));
    h.run();
    results.add(h.try_snapshot("bin_grid"));
    h.state_mut().bin.list = true;
    h.run();
    results.add(h.try_snapshot("bin_list"));
    h.state_mut().bin.query = "MKV".into();
    h.state_mut().bin.sort = 1;
    h.run();
    results.add(h.try_snapshot("bin_search"));
    h.state_mut().bin.confirm_remove = Some(jvid_core::model::AssetId(2));
    h.run();
    results.add(h.try_snapshot("bin_remove_in_use"));
}

/// Offline placeholders in the bin's grid and list and in the Preview.
#[test]
fn offline_snapshots() {
    if !gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    let mut h = bin_harness();
    let state = h.state_mut();
    state.missing = ["/m/clutch_round_3.mkv", "/m/music.flac", "/m/finale.mkv"]
        .map(PathBuf::from)
        .into();
    h.run();
    results.add(h.try_snapshot("offline_grid"));
    h.state_mut().bin.list = true;
    h.run();
    results.add(h.try_snapshot("offline_list"));
}

#[test]
fn hover_remove_button_uses_the_remove_flow() {
    use jvid_core::model::AssetId;
    let mut results = egui_kittest::SnapshotResults::new();
    let mut h = bin_harness();
    h.hover_at(egui::pos2(90.0, 160.0));
    h.run();
    if gpu() {
        results.add(h.try_snapshot("bin_grid_hover"));
    }
    h.state_mut().bin.list = true;
    // clutch_round_3.mkv, used by two clips: warns.
    h.hover_at(egui::pos2(100.0, 164.0));
    h.run();
    if gpu() {
        results.add(h.try_snapshot("bin_list_hover"));
    }
    h.get_by_label("Remove from project").click();
    h.run();
    assert_eq!(h.state().bin.confirm_remove, Some(AssetId(2)));
    h.get_by_label("Cancel").click();
    // arena_01.mkv is unused: removed at once.
    h.hover_at(egui::pos2(100.0, 132.0));
    h.run();
    h.get_by_label("Remove from project").click();
    h.run();
    assert!(h.state().history.project().asset(AssetId(1)).is_none());
}

#[test]
fn rename_keeps_the_extension() {
    use jvid_core::model::AssetId;
    let mut h = bin_harness();
    h.state_mut().bin.renaming = Some((AssetId(3), "pushed".into()));
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    let name = h
        .state()
        .history
        .project()
        .asset(AssetId(3))
        .unwrap()
        .display_name();
    assert_eq!(name, "pushed.mp4");
    h.state_mut().history.undo();
    let name = h
        .state()
        .history
        .project()
        .asset(AssetId(3))
        .unwrap()
        .display_name();
    assert_eq!(name, "push.mp4");
}

#[test]
fn rename_commits_on_click_away_and_esc_cancels() {
    use jvid_core::model::AssetId;
    let name = |h: &Harness<'_, App>| {
        let p = h.state().history.project();
        p.asset(AssetId(3)).unwrap().display_name()
    };
    let mut h = bin_harness();
    h.state_mut().bin.renaming = Some((AssetId(3), "gone".into()));
    h.run();
    h.key_press(egui::Key::Escape);
    h.run();
    assert_eq!(name(&h), "push.mp4");
    assert!(h.state().bin.renaming.is_none());

    h.state_mut().bin.renaming = Some((AssetId(3), "pushed".into()));
    h.run();
    // Far from the bin, in another panel.
    let away = egui::pos2(h.ctx.content_rect().right() - 20.0, 300.0);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: away,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        h.run();
    }
    assert_eq!(name(&h), "pushed.mp4");
    assert!(h.state().bin.renaming.is_none());
}

#[test]
fn remove_in_use_warns_then_removes_asset_and_clips() {
    let mut h = bin_harness();
    h.state_mut().bin.confirm_remove = Some(jvid_core::model::AssetId(2));
    h.run();
    h.get_by_label("Cancel").click();
    h.run();
    assert_eq!(h.state().history.project().assets().len(), 5);
    assert!(h.state().bin.confirm_remove.is_none());

    h.state_mut().bin.confirm_remove = Some(jvid_core::model::AssetId(2));
    h.run();
    h.get_by_label("Remove").click();
    h.run();
    let p = h.state().history.project();
    assert_eq!(p.assets().len(), 4);
    assert_eq!(p.clips_using(jvid_core::model::AssetId(2)), 0);
    h.state_mut().history.undo();
    assert_eq!(h.state().history.project().assets().len(), 5);
}

fn enabled(h: &Harness<'_, App>, label: &str) -> bool {
    !h.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn undo_redo_and_dirty_marker_follow_history() {
    let mut h = plain_harness();
    h.state_mut().on_welcome = false;
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
    assert_eq!(h.state().window_title, "Untitled • — jvid");

    let path = std::env::temp_dir().join(format!("jvid-dirty-{}.jvid", std::process::id()));
    h.state_mut().path = Some(path.clone());
    assert!(h.state_mut().save(false));
    h.run();
    std::fs::remove_file(&path).unwrap();
    assert!(!h.state().window_title.contains('•'));
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

    // A layout missing a panel falls back to the default.
    let restored = App::restore(Some(&storage));
    assert!(restored.dock.find_tab(&Tab::Inspector).is_some());

    // A rearranged layout with every panel is kept: the inspector joins the timeline.
    let mut app = App::restore(None);
    let inspector = app.dock.find_tab(&Tab::Inspector).unwrap();
    app.dock.remove_tab(inspector);
    let timeline = app.dock.find_tab(&Tab::Timeline).unwrap();
    app.dock.set_focused_node_and_surface(egui_dock::NodePath {
        surface: timeline.surface,
        node: timeline.node,
    });
    app.dock.push_to_focused_leaf(Tab::Inspector);
    let mut storage = Memory::default();
    eframe::App::save(&mut app, &mut storage);
    let restored = App::restore(Some(&storage));
    let (i, t) = (
        restored.dock.find_tab(&Tab::Inspector).unwrap(),
        restored.dock.find_tab(&Tab::Timeline).unwrap(),
    );
    assert_eq!((i.surface, i.node), (t.surface, t.node));
    for (_, node) in restored.dock.iter_all_nodes() {
        assert!(!node.is_leaf() || node.is_tab_bar_hidden());
    }
}

#[test]
fn corrupt_dock_layout_falls_back_to_default() {
    let mut storage = Memory::default();
    eframe::Storage::set_string(&mut storage, DOCK_KEY, "(old: \"layout\")".into());
    let restored = App::restore(Some(&storage));
    for tab in [Tab::Media, Tab::Preview, Tab::Inspector, Tab::Timeline] {
        assert!(restored.dock.find_tab(&tab).is_some());
    }
}

#[test]
fn focus_actions_focus_panels_and_missing_panels_are_no_ops() {
    let mut app = App::restore(None);
    let focused = |app: &mut App| app.dock.find_active_focused().map(|(_, t)| *t);
    app.run_action("focus_media", false);
    assert_eq!(focused(&mut app), Some(Tab::Media));
    app.run_action("focus_source", false);
    assert_eq!(focused(&mut app), Some(Tab::Preview));
    assert_eq!(app.preview_active, 0);
    app.run_action("focus_timeline", false);
    assert_eq!(focused(&mut app), Some(Tab::Timeline));
}

#[test]
fn program_draws_the_timeline_through_the_gpu() {
    use jvid_core::model::{NewClip, TrackKind};
    use jvid_core::time::Time;
    let Some(dir) = jvid_media::test_media::corpus() else {
        return;
    };
    if !gpu() {
        return;
    }
    jvid_media::ffmpeg::init().unwrap();
    let mut h = harness([1280.0, 720.0], 1.0);
    let path = dir.join("bars-bt709-limited.mp4");
    let probe = jvid_media::probe::probe(&path).unwrap();
    let duration = probe.duration;
    let history = &mut h.state_mut().history;
    history.apply(Command::Import(vec![(path, probe)])).unwrap();
    let Ok(jvid_core::history::Created::Track(track)) = history.apply(Command::AddTrack {
        kind: TrackKind::Video,
        name: "V1".into(),
    }) else {
        panic!()
    };
    let clip = NewClip {
        asset: AssetId(1),
        source_in: Time(0),
        source_out: duration,
        start: Time(0),
        link: None,
    };
    history.apply(Command::AddClip { track, clip }).unwrap();
    for _ in 0..500 {
        h.run();
        if h.state().preview.has_program_frame() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        h.state().preview.has_program_frame(),
        "decode never finished"
    );
    h.run();
    let shot = Shot::take(&mut h, "preview".into());
    let r = shot.panel(Tab::Preview);
    let y = shot.px(r.center().y);
    let row: Vec<Color32> = (shot.px(r.left())..shot.px(r.right()))
        .map(|x| shot.at(x, y))
        .collect();
    // Fitted bars, decoded and converted on the GPU, between Viewer black.
    for bar in [[191, 191, 0], [0, 191, 191], [191, 0, 191], [0, 0, 191]] {
        let want = Color32::from_rgb(bar[0], bar[1], bar[2]);
        assert!(
            row.iter().any(|&c| near(c, want)),
            "no {want:?} across the preview"
        );
    }
    assert!(row.contains(&color::VIEWER));
}

#[test]
fn program_transport_plays_and_seeks_the_timeline() {
    use jvid_core::time::{FLICKS_PER_SECOND, Time};
    if !gpu() {
        return;
    }
    let mut h = harness([1280.0, 720.0], 1.0);
    crate::playback::tests::add_solid(&mut h.state_mut().history, [1.0; 4], Time(0));
    h.run();
    h.get_by_label("Play (Space)").click();
    h.step();
    h.step();
    assert!(h.state().preview.program_playing());
    h.get_by_label("Stop (Space)").click();
    h.run();
    assert!(!h.state().preview.program_playing());
    h.get_by_label("Go to end (End)").click();
    h.run();
    // The last frame of the one-second solid, at the default 30 fps.
    let last = Time(FLICKS_PER_SECOND * 29 / 30);
    assert_eq!(h.state().timeline.playhead(), last);
    h.get_by_label("Go to start (Home)").click();
    h.run();
    assert_eq!(h.state().timeline.playhead(), Time(0));
}

#[test]
fn welcome_snapshots() {
    if !gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    for (size, ppp) in configs() {
        let mut h = welcome_harness(size, ppp);
        results.add(h.try_snapshot(format!("welcome_{}x{}_{ppp}x", size[0], size[1])));
    }
    // Recent projects (no posters: their media doesn't exist) and a fixed preset picked.
    let mut h = welcome_harness([1280.0, 720.0], 1.0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // Dates that read the same whenever the test runs: minutes ago, then last year's.
    let names = [
        "summer-road-trip",
        "product-demo",
        "gym-reel",
        "coastal-broll",
    ];
    h.state_mut().welcome.recent = names
        .iter()
        .zip([now - 10, now - 300, 1_760_227_200, 1_759_795_200])
        .map(|(n, opened)| welcome::Recent {
            path: format!("/p/{n}.jvid").into(),
            opened,
            poster: None,
        })
        .collect();
    h.get_by_label("9:16 · Vertical").click();
    h.run();
    results.add(h.try_snapshot("welcome_recent"));
}

fn fake_probe(p: &std::path::Path) -> Result<jvid_core::model::ProbeInfo, String> {
    let (w, h, fps) = match p.file_name().unwrap().to_str().unwrap() {
        "screen-1440p60.mp4" => (2560, 1440, 60),
        "phone.mp4" => (1080, 1920, 30),
        _ => (1280, 720, 25),
    };
    Ok(jvid_core::model::ProbeInfo {
        width: Some(w),
        height: Some(h),
        frame_rate: Some(jvid_core::time::Rate::new(fps, 1)),
        ..Default::default()
    })
}

/// No GPU: state-transition tests run everywhere.
fn plain_harness() -> Harness<'static, App> {
    let mut h = builder([1280.0, 720.0], 1.0).build_eframe(|cc| App::new(cc, &Default::default()));
    h.run();
    h
}

#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        Err("not read".into())
    }
}

/// Drops `files` on the welcome screen and waits for the import.
fn drop_on_welcome(files: &[&str]) -> Harness<'static, App> {
    let mut h = plain_harness();
    h.state_mut().importer = Importer::new(jvid_jobs::Pool::new(1), fake_probe);
    h.input_mut().dropped_files = files
        .iter()
        .map(|f| std::sync::Arc::new(Dropped(PathBuf::from("/m").join(f))) as _)
        .collect();
    h.step();
    assert!(!h.state().on_welcome);
    while h.state().importer.is_busy() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    h
}

#[test]
fn dropping_media_on_welcome_matches_the_first_clip() {
    use jvid_core::time::Rate;
    let settings = |h: &Harness<'_, App>| {
        let s = h.state().history.project().settings().clone();
        (s.width, s.height, s.frame_rate)
    };
    let h = drop_on_welcome(&["screen-1440p60.mp4"]);
    assert_eq!(settings(&h), (2560, 1440, Rate::new(60, 1)));
    assert!(!h.state().match_source);

    let mut h = drop_on_welcome(&["phone.mp4", "other.mp4"]);
    assert_eq!(settings(&h), (1080, 1920, Rate::new(30, 1)));
    assert_eq!(h.state().history.project().assets().len(), 2);
    // Later imports leave the settings alone.
    h.state_mut()
        .import([PathBuf::from("/m/screen-1440p60.mp4")]);
    while h.state().importer.is_busy() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    assert_eq!(settings(&h), (1080, 1920, Rate::new(30, 1)));
}

#[test]
fn new_project_uses_the_chosen_preset() {
    let mut h = plain_harness();
    assert_eq!(h.state().welcome.preset, 0, "Match source is the default");
    h.get_by_label("16:9 · 4K").click();
    h.get_by_label("60 fps").click();
    h.run();
    h.get_by_label("New project").click();
    h.run();
    let app = h.state();
    assert!(!app.on_welcome && !app.match_source && !app.history.is_dirty());
    let s = app.history.project().settings();
    assert_eq!((s.width, s.height, s.frame_rate.num), (3840, 2160, 60));

    // Match source keeps the default settings until media arrives.
    let mut h = plain_harness();
    h.get_by_label("New project").click();
    h.run();
    assert!(h.state().match_source && !h.state().on_welcome);
}

#[test]
fn source_actions_step_and_mark_the_open_clip_undoably() {
    use jvid_core::model::{AssetId, ProbeInfo};
    use jvid_core::time::{FLICKS_PER_SECOND, Rate, Time};
    let mut app = App::restore(None);
    let probe = ProbeInfo {
        duration: Time(10 * FLICKS_PER_SECOND),
        frame_rate: Some(Rate::new(25, 1)),
        ..Default::default()
    };
    let import = Command::Import(vec![("/m/a.mkv".into(), probe)]);
    app.history.apply(import).unwrap();
    let id = AssetId(1);
    app.preview.open(app.history.project(), id);
    app.run_action("focus_source", false);
    let marks = |app: &App| {
        let a = app.history.project().asset(id).unwrap();
        (a.in_point, a.out_point)
    };
    let frame = |n: i64| Rate::new(25, 1).time_floor(n);
    for a in [
        "step_forward",
        "step_forward",
        "mark_in",
        "go_to_end",
        "mark_out",
    ] {
        app.run_action(a, false);
    }
    assert_eq!(marks(&app), (Some(frame(2)), Some(frame(250))));
    // Marking out before the in clears the in.
    app.run_action("go_to_start", false);
    app.run_action("mark_out", false);
    assert_eq!(marks(&app), (None, Some(frame(1))));
    assert!(app.history.undo());
    assert_eq!(marks(&app), (Some(frame(2)), Some(frame(250))));
    // Go to in/out land on the marked frames: marking the other side there marks one frame.
    app.run_action("go_to_start", false);
    app.run_action("step_forward", false);
    app.run_action("mark_in", false);
    app.run_action("go_to_end", false);
    app.run_action("step_back", false);
    app.run_action("mark_out", false);
    assert_eq!(marks(&app), (Some(frame(1)), Some(frame(249))));
    app.run_action("go_to_in", false);
    app.run_action("mark_out", false);
    assert_eq!(marks(&app), (Some(frame(1)), Some(frame(2))));
    app.run_action("go_to_end", false);
    app.run_action("mark_out", false);
    app.run_action("go_to_out", false);
    app.run_action("mark_in", false);
    assert_eq!(marks(&app), (Some(frame(249)), Some(frame(250))));
    app.run_action("clear_in_out", false);
    assert_eq!(marks(&app), (None, None));
    // With another panel focused the keys aren't the Source tab's.
    app.run_action("focus_timeline", false);
    app.run_action("mark_in", false);
    assert_eq!(marks(&app), (None, None));
}

/// Release only. Perf (docs/perf.md): app construction to first rendered frame, then resident
/// memory after idling. In-process, so it leaves out exec and dynamic linking, and RSS includes
/// the test harness; run it alone (`-- perf_cold_start --test-threads 1`) for a clean number.
#[test]
fn perf_cold_start_and_idle_memory() {
    if cfg!(debug_assertions) || !gpu() {
        return;
    }
    let start = std::time::Instant::now();
    let mut h = welcome_harness([1280.0, 720.0], 1.0);
    h.render().expect("render");
    let cold = start.elapsed();
    for _ in 0..60 {
        h.run();
    }
    println!(
        "PERF cold start (in-process, to first frame): {:.0} ms (budget 1000 ms)",
        cold.as_secs_f64() * 1e3
    );
    // shortcut: Linux only (/proc); add macOS/Windows readings when the harness runs there.
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        let kb: u64 = status
            .lines()
            .find_map(|l| l.strip_prefix("VmRSS:"))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
            .expect("VmRSS");
        println!(
            "PERF idle memory (RSS after 60 idle frames): {} MB (budget 150 MB)",
            kb / 1024
        );
    }
}

#[test]
fn menu_actions_reach_the_timeline_from_any_panel_and_undo_is_an_action() {
    use jvid_core::model::{NewClip, ProbeInfo, TrackKind};
    use jvid_core::time::{FLICKS_PER_SECOND, Time};
    let mut app = App::restore(None);
    let probe = ProbeInfo {
        duration: Time(10 * FLICKS_PER_SECOND),
        width: Some(64),
        ..Default::default()
    };
    app.history
        .apply(Command::Import(vec![("/m/a.mkv".into(), probe)]))
        .unwrap();
    let kind = TrackKind::Video;
    let name = String::new();
    app.history.apply(Command::AddTrack { kind, name }).unwrap();
    let track = app.history.project().sequence().tracks()[0].id;
    let clip = NewClip {
        asset: app.history.project().assets()[0].id,
        source_in: Time(0),
        source_out: Time(FLICKS_PER_SECOND),
        start: Time(0),
        link: None,
    };
    app.history.apply(Command::AddClip { track, clip }).unwrap();
    app.run_action("focus_media", false);
    // Ctrl+A with the bin focused isn't the timeline's; the Edit menu's row is.
    app.run_action("select_all", false);
    assert!(app.timeline.selected().is_empty());
    app.run_action("select_all", true);
    assert_eq!(app.timeline.selected().len(), 1);

    app.run_action("undo", false);
    assert!(
        app.history.project().sequence().tracks()[0]
            .clips()
            .is_empty()
    );
    app.run_action("redo_alt", false);
    assert_eq!(
        app.history.project().sequence().tracks()[0].clips().len(),
        1
    );
}

#[test]
fn inspector_scrolls_to_its_last_field_in_a_small_window() {
    use jvid_core::time::Time;
    if !gpu() {
        return;
    }
    let mut h = harness([1024.0, 560.0], 1.0);
    let clip = crate::playback::tests::add_solid(&mut h.state_mut().history, [1.0; 4], Time(0));
    let app = h.state_mut();
    app.timeline
        .select_clip(app.history.project().sequence(), clip);
    h.run();
    let panel = Shot::take(&mut h, "inspector".into()).panel(Tab::Inspector);
    let last = h.get_by_label("LUT mix");
    assert!(last.rect().top() > panel.bottom(), "fits without scrolling");
    last.scroll_to_me();
    h.run();
    let r = h.get_by_label("LUT mix").rect();
    assert!(panel.contains_rect(r), "{r:?} not inside {panel:?}");
}

#[test]
fn file_menu_saves_exports_and_shows_keymap_shortcuts() {
    let mut h = plain_harness();
    h.state_mut().on_welcome = false;
    let path = std::env::temp_dir().join(format!("jvid-file-menu-{}.jvid", std::process::id()));
    h.state_mut().path = Some(path.clone());
    h.run();
    h.get_by_label("File").click();
    h.run();
    assert!(h.query_by_label("Open Recent ⏵").is_some());
    for id in [
        "new_project",
        "open_project",
        "save",
        "save_as",
        "import",
        "export",
        "quit",
    ] {
        let (name, shortcut) = h.state().keymap.label(id);
        assert!(!shortcut.is_empty(), "{id} unbound");
        let row = format!("{name} {shortcut}");
        assert!(h.query_by_label(&row).is_some(), "no {row:?} row");
    }
    h.get_by_label("Save Ctrl+S").click();
    h.run();
    assert!(path.exists());
    std::fs::remove_file(&path).unwrap();

    if !gpu() {
        return;
    }
    let mut h = harness([1280.0, 720.0], 1.0);
    let time = jvid_core::time::Time(0);
    crate::playback::tests::add_solid(&mut h.state_mut().history, [1.0; 4], time);
    h.run();
    h.get_by_label("File").click();
    h.run();
    h.get_by_label("Export Media Ctrl+M").click();
    h.run();
    assert!(h.query_by_label("Add to Queue").is_some());
}
