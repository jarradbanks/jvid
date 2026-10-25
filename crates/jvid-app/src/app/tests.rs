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
            if near(c, color::ground()) || near(c, color::panel()) {
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
        (x..)
            .find(|&x| near(self.at(x, y), color::border()))
            .unwrap()
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
        // Clear of the header's actions (the Program view's two compare toggles).
        let actions = 2.0 * (size::CONTROL + space::S4) + space::S8;
        let xs = self.px(r.left()) + 1..self.px(r.right() - actions) - 1;
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
                .find(|&x| near(s2.at(x, y), color::raised()))
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
        h.state_mut().preview_views = &["program"];
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

/// A bin of five assets, the second used by two clips and the third HDR.
fn bin_harness() -> Harness<'static, App> {
    use jvid_core::model::{AssetId, NewClip, ProbeInfo, StreamInfo, TrackKind, VideoInfo};
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
        (
            "/m/push.mp4".into(),
            ProbeInfo {
                streams: vec![StreamInfo::Video(VideoInfo {
                    color_transfer: Some("smpte2084".into()),
                    ..Default::default()
                })],
                ..video(729)
            },
        ),
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
    h.get_by_label("Remove from Project").click();
    h.run();
    assert_eq!(h.state().bin.confirm_remove, Some(AssetId(2)));
    h.get_by_label("Cancel").click();
    // arena_01.mkv is unused: removed at once.
    h.hover_at(egui::pos2(100.0, 132.0));
    h.run();
    h.get_by_label("Remove from Project").click();
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

/// Show Render Queue splits it off Timeline's right once; the layout keeps it; Close removes it
/// cleanly and showing it again puts it back beside Timeline.
#[test]
fn render_queue_docks_beside_timeline_and_reopens_there() {
    let mut app = App::restore(None);
    let leaves = |app: &App| {
        app.dock
            .iter_all_nodes()
            .filter(|(_, n)| n.is_leaf())
            .count()
    };
    let before = leaves(&app);
    assert!(app.dock.find_tab(&Tab::RenderQueue).is_none());
    for _ in 0..2 {
        app.show_queue();
    }
    let beside = |app: &App| {
        let (q, t) = (
            app.dock.find_tab(&Tab::RenderQueue).unwrap(),
            app.dock.find_tab(&Tab::Timeline).unwrap(),
        );
        // A split's two children sit at 2n+1 and 2n+2, so the queue is Timeline's right sibling.
        q.surface == t.surface && q.node.0 == t.node.0 + 1 && t.node.0 % 2 == 1
    };
    assert!(beside(&app));
    assert_eq!(leaves(&app), before + 1);
    assert_eq!(
        app.dock.find_active_focused().map(|(_, t)| *t),
        Some(Tab::RenderQueue)
    );

    let mut storage = Memory::default();
    eframe::App::save(&mut app, &mut storage);
    let mut app = App::restore(Some(&storage));
    assert!(beside(&app));
    for (_, node) in app.dock.iter_all_nodes() {
        assert!(!node.is_leaf() || node.is_tab_bar_hidden());
    }

    let ctx = egui::Context::default();
    app.queue_action(&ctx, Some(QueueAction::Close));
    assert!(app.dock.find_tab(&Tab::RenderQueue).is_none());
    assert_eq!(leaves(&app), before);
    // A saved layout without the queue is still whole.
    let mut storage = Memory::default();
    eframe::App::save(&mut app, &mut storage);
    let mut app = App::restore(Some(&storage));
    app.queue_action(&ctx, Some(QueueAction::Show));
    assert!(beside(&app));
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
    // The decode, filmstrip and thumbnail workers repaint as their results land, so `run` can't
    // settle while they're busy (it panicked on loaded CI): step until they've all finished. A
    // quiet `try_run` alone can fall between two results, so the job pool must be empty too.
    for _ in 0..500 {
        h.step();
        if h.state().preview.has_program_frame()
            && h.try_run().is_ok()
            && h.state().timeline.jobs_idle()
        {
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
    h.get_by_label("Go to End (End)").click();
    h.run();
    // The last frame of the one-second solid, at the default 30 fps.
    let last = Time(FLICKS_PER_SECOND * 29 / 30);
    assert_eq!(h.state().timeline.playhead(), last);
    h.get_by_label("Go to Start (Home)").click();
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
    assert_eq!(h.state().welcome.preset, 0, "Match Source is the default");
    h.get_by_label("16:9 · 4K").click();
    h.get_by_label("60 fps").click();
    h.run();
    h.get_by_label("New Project").click();
    h.run();
    let app = h.state();
    assert!(!app.on_welcome && !app.match_source && !app.history.is_dirty());
    let s = app.history.project().settings();
    assert_eq!((s.width, s.height, s.frame_rate.num), (3840, 2160, 60));

    // Match source keeps the default settings until media arrives.
    let mut h = plain_harness();
    h.get_by_label("New Project").click();
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
fn inspector_tabs_fill_the_header() {
    use jvid_core::time::Time;
    if !gpu() {
        return;
    }
    let ends = |h: &mut Harness<'_, App>, last: &str| {
        let panel = Shot::take(h, "inspector".into()).panel(Tab::Inspector);
        let right = h
            .query_all_by_label(last)
            .map(|n| n.rect())
            .find(|r| panel.contains_rect(*r) && r.top() < panel.top() + size::HEADER)
            .unwrap()
            .right();
        assert!(
            (panel.right() - space::S4 - right).abs() <= 1.0,
            "{right} vs {panel:?}"
        );
    };
    // Wide enough that 160px tabs would stop short.
    // Video, Effects.
    let mut h = harness([1920.0, 1080.0], 1.0);
    let clip = crate::playback::tests::add_solid(&mut h.state_mut().history, [1.0; 4], Time(0));
    let app = h.state_mut();
    app.timeline
        .select_clip(app.history.project().sequence(), clip);
    h.run();
    ends(&mut h, "Effects");
    // Text, Video, Effects.
    let (mut h, _, _) = title_harness_sized([1920.0, 1080.0]);
    ends(&mut h, "Effects");
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
    let last = h.get_by_label("LUT");
    assert!(last.rect().top() > panel.bottom(), "fits without scrolling");
    last.scroll_to_me();
    h.run();
    let r = h.get_by_label("LUT").rect();
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

#[test]
fn edit_menu_opens_preferences_and_the_project_menu_keeps_project_settings() {
    let mut h = plain_harness();
    h.state_mut().on_welcome = false;
    h.run();
    h.get_by_label("Untitled").click();
    h.run();
    assert!(h.query_by_label("Project Settings…").is_some());
    assert!(h.query_by_label("Preferences…").is_none());
    assert!(h.query_by_label("Settings…").is_none());
    h.key_press(egui::Key::Escape);
    h.run();

    h.get_by_label("Edit").click();
    h.run();
    h.get_by_label("Preferences…").click();
    h.run();
    assert!(h.state().preferences.is_open());
    assert!(h.query_by_label("Language").is_some());
}

#[test]
fn help_menu_about_credits_the_sample_media() {
    let mut h = plain_harness();
    h.state_mut().on_welcome = false;
    h.run();
    h.get_by_label("Help").click();
    h.run();
    h.get_by_label("About jvid…").click();
    h.run();
    assert!(h.query_by_label_contains("Sample Media").is_some());
    for credit in crate::samples::credits() {
        assert!(
            h.query_by_label_contains(&credit).is_some(),
            "About is missing {credit}"
        );
    }
    h.get_by_label("OK").click();
    h.run();
    assert!(h.state().prompts.is_empty());
}

#[test]
fn about_opens_the_third_party_notices() {
    let mut h = plain_harness();
    h.state_mut().on_welcome = false;
    h.state_mut().prompts.push(Prompt::About);
    h.run();
    h.get_by_label("Third-Party Notices").click();
    h.run();
    assert_eq!(h.state().prompts, [Prompt::Notices]);
    assert!(
        h.query_by_label_contains("jvid Third-Party Notices")
            .is_some()
    );
    // A move-and-click in one frame misses here, so hover first as a real pointer does.
    h.get_by_label("OK").hover();
    h.run();
    h.get_by_label("OK").click();
    h.run();
    assert!(h.state().prompts.is_empty());
}

#[test]
fn project_settings_turn_on_hdr_output() {
    if !gpu() {
        return;
    }
    let mut h = harness([1280.0, 720.0], 1.0);
    h.get_by_label("Untitled").click();
    h.run();
    h.get_by_label("Project Settings…").click();
    h.run();
    // Without an HEVC encoder the checkbox is disabled and clicks do nothing.
    h.state_mut().settings_dialog.hdr10 = false;
    h.run();
    h.get_by_label("HEVC 10-Bit PQ").click();
    h.run();
    assert!(!h.state().settings_dialog.draft.as_ref().unwrap().hdr_output);
    h.state_mut().settings_dialog.hdr10 = true;
    h.run();
    h.get_by_label("HEVC 10-Bit PQ").click();
    h.run();
    h.snapshot("project_settings_hdr");
    h.get_by_label("Apply").click();
    h.run();
    assert!(h.state().history.project().settings().hdr_output);
}

#[test]
fn dragging_a_title_preset_onto_v2_places_a_text_clip() {
    if !gpu() {
        return;
    }
    use jvid_core::model::TrackKind;
    let mut h = harness([1280.0, 720.0], 1.0);
    for name in ["V1", "V2"] {
        let kind = TrackKind::Video;
        let name = name.into();
        h.state_mut()
            .history
            .apply(Command::AddTrack { kind, name })
            .unwrap();
    }
    h.state_mut().bin.tab = 2;
    h.run();
    let from = h.get_by_label("Title").rect().center();
    // Video tracks stack upward, so V2's header (its Lock toggle) is the top one.
    let locks = h.get_all_by_label("Lock").map(|n| n.rect());
    let v2 = locks.min_by(|a, b| a.top().total_cmp(&b.top())).unwrap();
    let to = egui::pos2(v2.right() + 300.0, v2.center().y);
    h.hover_at(from);
    h.drag_at(from);
    h.run();
    for i in 1..=10 {
        h.hover_at(from + (to - from) * (i as f32 / 10.0));
        h.run();
    }
    h.drop_at(to);
    h.run();
    h.run();
    let p = h.state().history.project();
    let v2 = p
        .sequence()
        .tracks()
        .iter()
        .find(|t| t.name == "V2")
        .unwrap();
    let clip = v2.clips().first().expect("a clip on V2");
    assert!(p.asset(clip.asset).unwrap().text.is_some());
}

#[test]
fn clicking_a_preset_shows_its_asset_in_the_media_tab() {
    if !gpu() {
        return;
    }
    let mut h = harness([1280.0, 720.0], 1.0);
    h.state_mut().bin.tab = 2;
    h.run();
    h.get_by_label("Title").click();
    h.run();
    h.run();
    let id = h.state().history.project().assets().last().map(|a| a.id);
    assert!(id.is_some());
    assert_eq!((h.state().bin.tab, h.state().bin.selected), (0, id));
}

#[test]
fn each_text_tile_shows_its_preset_on_the_frame() {
    if !gpu() {
        return;
    }
    let mut h = harness([1280.0, 720.0], 1.0);
    h.state_mut().bin.tab = 2;
    h.run();
    let img = h.render().unwrap();
    // Each tile's thumbnail: inset 4pt, 16:9 across the tile.
    let thumb = |name: &str| {
        let r = h.get_by_label(name).rect();
        let w = r.width() - 8.0;
        Rect::from_min_size(r.min + egui::vec2(4.0, 4.0), egui::vec2(w, w * 9.0 / 16.0))
    };
    let pixels = |r: Rect| -> Vec<[u8; 4]> {
        let (x0, y0) = (r.left().ceil() as u32, r.top().ceil() as u32);
        let (w, h) = (r.width() as u32 - 1, r.height() as u32 - 1);
        (y0..y0 + h)
            .flat_map(|y| (x0..x0 + w).map(move |x| (x, y)))
            .map(|(x, y)| img.get_pixel(x, y).0)
            .collect()
    };
    let [title, lower, caption] = ["Title", "Lower Third", "Caption"].map(|n| pixels(thumb(n)));
    assert!(lower.iter().any(|p| p[..3] != [0, 0, 0]), "the frame draws");
    // Same frame, same size: where they clearly differ is where a preset lands. The lower third's box
    // draws whatever the fonts, and sits low on the frame.
    let rows = |a: &[[u8; 4]], b: &[[u8; 4]]| {
        let w = thumb("Title").width() as usize - 1;
        let n = a.len() / w;
        (0..n)
            .filter(|y| {
                // Columns differ by a subpixel of resampling; a preset by far more.
                let far = |(p, q): (&[u8; 4], &[u8; 4])| (0..3).any(|c| p[c].abs_diff(q[c]) > 48);
                a[y * w..][..w].iter().zip(&b[y * w..][..w]).any(far)
            })
            .map(|y| y as f32 / n as f32)
            .collect::<Vec<_>>()
    };
    let low = rows(&lower, &caption);
    assert!(!low.is_empty(), "the lower third draws");
    assert!(low.iter().all(|y| *y > 0.5), "below the middle: {low:?}");
    if !jvid_render::text::families().is_empty() {
        assert!(
            rows(&title, &caption)
                .iter()
                .any(|y| (0.4..0.6).contains(y)),
            "the title, centred"
        );
    }
}

#[test]
fn effect_rows_show_each_effect_and_hover_only_fills() {
    if !gpu() {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("jvid-fx-rows-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    crate::effects::scan_for_test(&tmp);
    let mut h = harness([1280.0, 720.0], 1.0);
    h.state_mut().bin.tab = 1;
    h.run();
    let names = [
        "Chroma Key",
        "Chromatic Aberration",
        "Film Grain",
        "Gaussian Blur",
        "Pixelate",
        "Vignette",
    ];
    let nodes = |h: &Harness<'_, App>| {
        h.root()
            .children_recursive()
            .map(|n| n.accesskit_node())
            .map(|n| format!("{:?} {:?} {:?}", n.role(), n.label(), n.bounding_box()))
            .collect::<Vec<_>>()
    };
    // Apply and More are there before any hover, and Apply waits for a selected clip.
    h.get_by_label("New Effect").hover();
    h.run();
    let before = nodes(&h);
    let apply: Vec<_> = h.get_all_by_label("Apply to Selected Clips").collect();
    assert_eq!(apply.len(), names.len());
    assert!(apply.iter().all(|n| n.accesskit_node().is_disabled()));
    assert_eq!(h.get_all_by_label("More").count(), names.len());
    h.get_by_label("Vignette").hover();
    h.run();
    assert_eq!(nodes(&h), before, "hover moved or added something");
    h.snapshot("bin_effects_hover");

    // Each row's 80x45 thumbnail, 4pt in and centred, is drawn and differs from the others.
    let img = h.render().unwrap();
    let thumb = |name: &str| {
        let r = h.get_by_label(name).rect();
        Rect::from_center_size(
            egui::pos2(r.left() + 4.0 + 40.0, r.center().y),
            egui::vec2(78.0, 43.0),
        )
    };
    let pixels = |r: Rect| -> Vec<[u8; 4]> {
        let (x0, y0) = (r.left().ceil() as u32, r.top().ceil() as u32);
        (y0..y0 + r.height() as u32)
            .flat_map(|y| (x0..x0 + r.width() as u32).map(move |x| (x, y)))
            .map(|(x, y)| img.get_pixel(x, y).0)
            .collect()
    };
    let thumbs = names.map(|n| pixels(thumb(n)));
    for (n, t) in names.iter().zip(&thumbs) {
        assert!(t.iter().any(|p| p[..3] != [0, 0, 0]), "{n} isn't drawn");
    }
    let n = names.len();
    for i in 0..n {
        let (a, b) = (&thumbs[i], &thumbs[(i + 1) % n]);
        let far = a
            .iter()
            .zip(b)
            .filter(|(p, q)| (0..3).any(|c| p[c].abs_diff(q[c]) > 8))
            .count();
        assert!(
            far > a.len() / 20,
            "{} looks like {}",
            names[i],
            names[(i + 1) % n]
        );
    }

    // The Media tab's grid toggle shows them as tiles, Apply only over the hovered one.
    h.get_by_label("Grid View").click();
    h.run();
    h.get_by_label("Film Grain").hover();
    h.run();
    assert!(h.state().bin.fx_grid);
    assert_eq!(h.get_all_by_label("Apply to Selected Clips").count(), 1);
    h.snapshot("bin_effects_grid");
    h.get_by_label("List View").click();
    h.run();

    // Selecting a video clip turns Apply on.
    let clip = crate::playback::tests::add_solid(&mut h.state_mut().history, [1.0; 4], Time(0));
    let app = h.state_mut();
    app.timeline
        .select_clip(app.history.project().sequence(), clip);
    h.run();
    let apply: Vec<_> = h.get_all_by_label("Apply to Selected Clips").collect();
    assert!(apply.iter().all(|n| !n.accesskit_node().is_disabled()));
    std::fs::remove_dir_all(&tmp).unwrap();
}

/// A selected title in a 1280x720 app, its clip id and the Program frame's center.
fn title_harness() -> (Harness<'static, App>, jvid_core::model::ClipId, egui::Pos2) {
    title_harness_sized([1280.0, 720.0])
}

fn title_harness_sized(
    size: [f32; 2],
) -> (Harness<'static, App>, jvid_core::model::ClipId, egui::Pos2) {
    use jvid_core::history::Created;
    use jvid_core::model::{NewClip, Text, TrackKind};
    use jvid_core::time::{FLICKS_PER_SECOND, Time};
    // Steps short enough for two clicks to make a double-click.
    let mut h = builder(size, 1.0)
        .with_step_dt(0.05)
        .wgpu()
        .build_eframe(|cc| App::new(cc, &Default::default()));
    h.state_mut().on_welcome = false;
    h.run();
    let history = &mut h.state_mut().history;
    // A box makes the hit independent of the installed fonts.
    let text = Text {
        box_color: [0.0, 0.0, 0.0, 1.0],
        ..Text::default()
    };
    let Ok(Created::Asset(asset)) = history.apply(Command::AddText {
        name: "Title".into(),
        text,
    }) else {
        panic!("text")
    };
    let Ok(Created::Track(track)) = history.apply(Command::AddTrack {
        kind: TrackKind::Video,
        name: "V1".into(),
    }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset,
        source_in: Time(0),
        source_out: Time(FLICKS_PER_SECOND),
        start: Time(0),
        link: None,
    };
    let Ok(Created::Clip(id)) = history.apply(Command::AddClip { track, clip }) else {
        panic!("clip")
    };
    h.run();
    let app = h.state_mut();
    app.timeline
        .select_clip(app.history.project().sequence(), id);
    h.run();
    let shot = Shot::take(&mut h, "title".into());
    let r = shot.panel(Tab::Preview);
    let center = egui::pos2(r.center().x, r.center().y - size::HEADER / 2.0);
    (h, id, center)
}

/// #265: with nothing armed, the transport row's Record button adds an audio track, arms it
/// and records there; Shift+R ends the same take and the clip lands at the record start.
#[test]
fn record_button_records_into_a_new_armed_track_and_shift_r_stops_it() {
    use jvid_core::model::TrackKind;
    if !gpu() {
        return;
    }
    let (mut h, _, _) = title_harness();
    let dir = std::env::temp_dir().join(format!("jvid-record-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mic = crate::record::tests::FakeMic::default();
    let app = h.state_mut();
    app.path = Some(dir.join("p.jvid"));
    app.input = Some(Box::new(mic.clone()));
    let tracks = |h: &Harness<'_, App>| h.state().history.project().sequence().tracks().to_vec();
    let before = tracks(&h).len();
    assert_eq!(h.state().timeline.armed, None);

    h.get_by_label("Record Voiceover (Shift+R)").click();
    // A take repaints every frame, so step rather than run to idle.
    h.run_steps(2);
    let t = tracks(&h);
    assert_eq!(t.len(), before + 1);
    let new = t.iter().find(|t| t.kind == TrackKind::Audio).unwrap().id;
    assert_eq!(h.state().timeline.armed, Some(new));
    assert!(h.state().take.is_some());
    assert!(h.state().preview.recording.is_some());
    // The button turns into Stop with the same take behind it.
    h.get_by_label("Stop Recording (Shift+R)");

    mic.0.lock().unwrap().as_ref().unwrap().push(&[0.25; 4_800]);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::R);
    h.run_steps(2);
    assert!(h.state().take.is_none() && h.state().preview.recording.is_none());
    let project = h.state().history.project();
    let [clip] = project.sequence().track(new).unwrap().clips() else {
        panic!("one clip on the new track")
    };
    assert_eq!(clip.start, jvid_core::time::Time(0));
    // The parked playhead is on the take, so Program keeps it open to scrub; Windows won't
    // remove an open file.
    drop(h);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_selected_title_puts_text_video_and_effects_tabs_in_the_inspector_header() {
    if !gpu() {
        return;
    }
    let (mut h, _, _) = title_harness();
    // The bin has its own Text and Effects tabs, and the Text tab a Text section.
    let tabs = |name| h.query_all_by_label(name).count();
    assert_eq!(
        ["Text", "Video", "Audio", "Effects"].map(tabs),
        [3, 1, 0, 2]
    );
    let s = Shot::take(&mut h, "title".into());
    let (preview, inspector) = (s.panel(Tab::Preview), s.panel(Tab::Inspector));
    assert_eq!(
        s.column(inspector)[1],
        s.column(preview)[1],
        "header divider"
    );
    assert_eq!(s.label(inspector).2, s.label(preview).2, "header baseline");
}

#[test]
fn double_clicking_a_title_in_program_edits_it_and_space_types() {
    if !gpu() {
        return;
    }
    let (mut h, _, at) = title_harness();
    h.hover_at(at);
    h.step();
    // Both clicks in one step, inside the double-click window.
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
    }
    h.run();
    assert!(crate::handles::editing(&h.ctx));
    h.key_press(egui::Key::End);
    // Winit sends Space as a key and as text.
    h.key_press(egui::Key::Space);
    h.event(egui::Event::Text(" ".into()));
    h.run();
    let text = |h: &Harness<'_, App>| {
        let p = h.state().history.project();
        let a = p.assets().iter().find_map(|a| a.text.clone());
        a.unwrap().content
    };
    assert_eq!(text(&h), "Title ");
    assert!(
        !h.state().preview.playing(),
        "Space typed instead of playing"
    );
    h.key_press(egui::Key::Escape);
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run();
    assert_eq!(text(&h), "Title", "Ctrl+Z undoes the edit");
}

/// An edited project at a temp path, with a close request queued.
fn closing_harness(name: &str) -> (Harness<'static, App>, PathBuf) {
    let mut h = harness([1280.0, 720.0], 1.0);
    let path = std::env::temp_dir().join(format!("jvid-{name}-{}.jvid", std::process::id()));
    let app = h.state_mut();
    app.path = Some(path.clone());
    app.history
        .apply(Command::SetSettings(ProjectSettings {
            width: 640,
            ..Default::default()
        }))
        .unwrap();
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    h.run();
    h.get_by_label("Unsaved Changes");
    assert!(!h.state().closing);
    (h, path)
}

#[test]
fn close_with_unsaved_changes_asks_first() {
    if !gpu() {
        return;
    }
    let (mut h, _) = closing_harness("cancel");
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().prompts.is_empty() && !h.state().closing);
    assert!(h.state().history.is_dirty());

    let (mut h, path) = closing_harness("dont-save");
    h.get_by_label("Don't Save").click();
    h.run();
    assert!(h.state().closing && !path.exists());

    let (mut h, path) = closing_harness("save");
    h.get_by_label("Save").click();
    h.run();
    assert!(h.state().closing && path.exists() && !h.state().history.is_dirty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn dialog_keys_enter_saves_esc_cancels() {
    if !gpu() {
        return;
    }
    let (mut h, _) = closing_harness("esc");
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(h.state().prompts.is_empty() && !h.state().closing);

    let (mut h, path) = closing_harness("enter");
    h.key_press(egui::Key::Enter);
    h.run();
    assert!(h.state().closing && path.exists());
    let _ = std::fs::remove_file(path);
}

/// The Scopes tab over a red solid at the playhead: one dot on the red target, red's luma
/// line across the waveform, and spikes at 255 in red and 0 in green and blue.
#[test]
fn scopes_snapshot() {
    if !gpu() {
        return;
    }
    let mut h = harness([1280.0, 720.0], 1.0);
    let history = &mut h.state_mut().history;
    crate::playback::tests::add_solid(history, [1.0, 0.0, 0.0, 1.0], jvid_core::time::Time(0));
    h.state_mut().bin.tab = 3;
    for _ in 0..6 {
        h.step();
        h.render().unwrap();
    }
    h.snapshot("bin_scopes");
}

/// In a narrow bin the Scopes tab moves into the chevron menu and still opens from there.
#[test]
fn a_narrow_bin_opens_scopes_from_the_tab_menu() {
    if !gpu() {
        return;
    }
    let mut h = harness([960.0, 720.0], 1.0);
    h.run();
    assert!(h.query_by_label("Scopes").is_none(), "folded into the menu");
    h.get_by_label("More Tabs").click();
    h.run();
    h.get_by_label("Scopes").click();
    h.run();
    assert_eq!(h.state().bin.tab, 3);
}

/// A themes folder holding the bundled themes and a file that doesn't parse.
fn themes_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jvid-themes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    suite_ui::themes::install_bundled(&dir).unwrap();
    std::fs::write(dir.join("neon.toml"), "panel = \"#12345\"\n").unwrap();
    dir
}

/// Input devices as a streamer's Windows machine names them.
const DEVICES: [&str; 13] = [
    "Microphone Array (Realtek(R) Audio) — Intel® Smart Sound Technology for Digital Microphones",
    "Headset Microphone (Arctis Nova 7 Wireless)",
    "Line In (Focusrite Scarlett 2i2 4th Gen)",
    "Microphone (NVIDIA Broadcast)",
    "Microphone (Steam Streaming Microphone)",
    "CABLE Output (VB-Audio Virtual Cable)",
    "Stereo Mix (Realtek(R) Audio)",
    "Digital Audio Interface (USB Capture HDMI 4K+)",
    "Microphone (Razer Seiren Mini)",
    "Microphone (Logitech BRIO)",
    "Voicemeeter Out B1 (VB-Audio Voicemeeter VAIO)",
    "Microphone (OBS Virtual Audio)",
    "Line (Elgato Wave Link Stream)",
];

#[test]
fn preferences_fit_the_minimum_window() {
    if !gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    let mut h = harness([960.0, 600.0], 1.0);
    let dir = themes_dir("min");
    let devices = DEVICES.map(String::from).to_vec();
    h.state_mut().preferences.open(devices, Some(dir.clone()));
    h.run();
    for name in [
        "Dark",
        "Light",
        "High Contrast",
        "Warm Dark",
        "Soft Light",
        "Neon",
    ] {
        assert!(h.query_by_label(name).is_some(), "no {name} card");
    }
    results.add(h.try_snapshot("preferences_general"));

    h.get_by_label("Audio").click();
    h.run();
    h.get_by_label("System Default").click();
    h.run();
    let long = DEVICES[0];
    h.get_by_label(long).hover();
    for _ in 0..40 {
        h.step();
    }
    // The row and its tooltip.
    assert_eq!(
        h.query_all_by_label(long).count(),
        2,
        "no full name on hover"
    );
    let done = h.get_by_label("Done").rect();
    let screen = h.ctx.content_rect();
    let menu = h.get_by_label(DEVICES[1]).rect();
    assert!(screen.contains_rect(menu), "{menu:?} outside the window");
    assert!(
        menu.max.x <= done.max.x,
        "row wider than the dialog: {menu:?}"
    );
    results.add(h.try_snapshot("preferences_input_devices"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn editor_in_light_and_high_contrast() {
    if !gpu() {
        return;
    }
    let mut results = egui_kittest::SnapshotResults::new();
    for theme in ["light", "high-contrast"] {
        // A selected clip fills the inspector, so every panel shows its controls.
        let (mut h, _, _) = title_harness();
        h.state_mut().theme = suite_ui::themes::LiveTheme::new(theme);
        h.run();
        let want = suite_ui::themes::BUILT_IN
            .iter()
            .find(|t| t.0 == theme)
            .unwrap()
            .1;
        assert_eq!(h.ctx.global_style().visuals.panel_fill, want.panel);
        results.add(h.try_snapshot(format!("editor_{theme}")));
    }
}

/// #266: with the Timeline focused, Up over an EQ band nudges the band and leaves the
/// playhead where it was, instead of also jumping to the previous edit.
#[test]
fn up_over_an_eq_band_nudges_it_and_not_the_playhead() {
    use jvid_core::history::Created;
    use jvid_core::model::{NewClip, ProbeInfo, TrackKind};
    use jvid_core::time::{FLICKS_PER_SECOND as S, Time};
    if !gpu() {
        return;
    }
    let mut h = harness([1280.0, 1000.0], 1.0);
    let history = &mut h.state_mut().history;
    let probe = ProbeInfo {
        duration: Time(60 * S),
        sample_rate: Some(48_000),
        audio_streams: 1,
        ..Default::default()
    };
    history
        .apply(Command::Import(vec![("/nonexistent/a.wav".into(), probe)]))
        .unwrap();
    let asset = history.project().assets()[0].id;
    let Ok(Created::Track(track)) = history.apply(Command::AddTrack {
        kind: TrackKind::Audio,
        name: "A1".into(),
    }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset,
        source_in: Time(0),
        source_out: Time(10 * S),
        start: Time(0),
        link: None,
    };
    let Ok(Created::Clip(id)) = history.apply(Command::AddClip { track, clip }) else {
        panic!("clip")
    };
    let app = h.state_mut();
    app.timeline
        .select_clip(app.history.project().sequence(), id);
    app.timeline.seek(Time(5 * S));
    app.run_action("focus_timeline", false);
    h.run();
    let band = |h: &Harness<'_, App>| {
        let seq = h.state().history.project().sequence();
        let clip = &seq.track(track).unwrap().clips()[0];
        clip.transform.audio_fx.eq.bands[5]
    };
    let at = h
        .get_by_role_and_label(egui::accesskit::Role::Slider, "1 kHz")
        .rect()
        .center();
    h.hover_at(at);
    h.run();
    h.key_press(egui::Key::ArrowUp);
    h.run();
    assert_eq!(band(&h), 0.5);
    assert_eq!(h.state().timeline.playhead(), Time(5 * S));
}

/// Speech-to-text stand-in: "Hello there. This is a test." from 0.1 s, or, when `slow`, no
/// words until cancelled. Keeps the options it was asked with.
pub(super) struct FakeTranscriber {
    pub(super) slow: bool,
    pub(super) asked: std::sync::Mutex<Vec<jvid_transcribe::Options>>,
}

impl jvid_transcribe::Transcriber for FakeTranscriber {
    fn transcribe(
        &self,
        _: Vec<f32>,
        opts: &jvid_transcribe::Options,
        ctx: &jvid_jobs::Ctx,
    ) -> Result<jvid_transcribe::Transcript, jvid_transcribe::Error> {
        use jvid_core::time::{FLICKS_PER_SECOND as S, Time};
        self.asked.lock().unwrap().push(opts.clone());
        while self.slow && !ctx.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if ctx.is_cancelled() {
            return Err(jvid_transcribe::Error::Cancelled);
        }
        let words = "Hello there. This is a test.".split(' ').enumerate();
        let words = words.map(|(i, w)| jvid_transcribe::Word {
            text: w.into(),
            start: Time(S / 10 + i as i64 * S / 4),
            end: Time(S / 10 + (i + 1) as i64 * S / 4),
            confidence: 1.0,
        });
        Ok(jvid_transcribe::Transcript {
            language: Some("en".into()),
            words: words.collect(),
        })
    }
}

/// The voice sample on an audio track, selected, with `fake` as the loaded Fast model.
fn subtitles_harness(
    fake: std::sync::Arc<dyn jvid_transcribe::Transcriber>,
) -> Harness<'static, App> {
    use jvid_core::history::Created;
    use jvid_core::model::{NewClip, TrackKind};
    use jvid_core::time::Time;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/samples/voice.m4a");
    jvid_media::ffmpeg::init().unwrap();
    let probe = jvid_media::probe::probe(&path).unwrap();
    let mut h = plain_harness();
    let app = h.state_mut();
    app.on_welcome = false;
    let duration = probe.duration;
    app.history
        .apply(Command::Import(vec![(path, probe)]))
        .unwrap();
    let (kind, name) = (TrackKind::Audio, String::new());
    let Ok(Created::Track(track)) = app.history.apply(Command::AddTrack { kind, name }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset: app.history.project().assets()[0].id,
        source_in: Time(0),
        source_out: duration,
        start: Time(0),
        link: None,
    };
    let Ok(Created::Clip(id)) = app.history.apply(Command::AddClip { track, clip }) else {
        panic!("clip")
    };
    app.timeline
        .select_clip(app.history.project().sequence(), id);
    app.subtitles.engine = Some((jvid_transcribe::models::Model::Base, fake));
    h.run();
    h.get_by_label("Timeline").click();
    h.run();
    h.get_by_label("Generate Subtitles… Ctrl+Alt+G").click();
    h.run();
    assert!(h.state().subtitles.is_open());
    h
}

/// Acceptance: a fake transcriber drives Timeline > Generate Subtitles end to end, the
/// dialog's choices reach it, and one undo removes the track.
#[test]
fn generate_subtitles_adds_a_caption_track_that_one_undo_removes() {
    let fake = std::sync::Arc::new(FakeTranscriber {
        slow: false,
        asked: Default::default(),
    });
    let mut h = subtitles_harness(fake.clone());
    let before = h.state().history.project().clone();
    h.get_by_label("Translate to English").click();
    h.run();
    h.get_by_label("Generate").click();
    h.step();
    let start = std::time::Instant::now();
    while h.state().subtitles.is_open() {
        assert!(start.elapsed().as_secs() < 30, "never finished");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    let asked = fake.asked.lock().unwrap().clone();
    let [opts] = &asked[..] else {
        panic!("{asked:?}")
    };
    assert!(opts.translate && opts.language.is_none());

    let project = h.state().history.project();
    let track = project.sequence().tracks().iter().find(|t| t.subtitles);
    assert_eq!(track.expect("a subtitle track").name, "Subtitles");
    let cues: Vec<String> = project
        .subtitle_cues()
        .into_iter()
        .map(|c| c.text)
        .collect();
    assert_eq!(cues, ["Hello there.", "This is a test."]);
    assert!(h.state_mut().history.undo());
    assert_eq!(h.state().history.project(), &before);
}

/// Closing the dialog mid-run cancels the job: nothing keeps running and nothing lands.
#[test]
fn closing_generate_subtitles_cancels_the_job() {
    let fake = std::sync::Arc::new(FakeTranscriber {
        slow: true,
        asked: Default::default(),
    });
    let mut h = subtitles_harness(fake.clone());
    let before = h.state().history.project().clone();
    h.get_by_label("Generate").click();
    h.step();
    let start = std::time::Instant::now();
    while fake.asked.lock().unwrap().is_empty() {
        assert!(start.elapsed().as_secs() < 30, "never started");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    assert!(h.query_by_label("Transcribing").is_some());
    h.get_by_label("Cancel").click();
    h.step();
    assert!(!h.state().subtitles.is_open());
    // The job winds down and its pool goes idle.
    let jobs = h.state().subtitles.jobs.as_ref().unwrap();
    let start = std::time::Instant::now();
    while !jobs.is_idle() {
        assert!(start.elapsed().as_secs() < 1, "still running");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().history.project(), &before);
}

struct CrashingTranscriber;

impl jvid_transcribe::Transcriber for CrashingTranscriber {
    fn transcribe(
        &self,
        _: Vec<f32>,
        _: &jvid_transcribe::Options,
        _: &jvid_jobs::Ctx,
    ) -> Result<jvid_transcribe::Transcript, jvid_transcribe::Error> {
        panic!("engine crashed")
    }
}

/// A job that dies without a result ends the run with an error instead of spinning forever.
#[test]
fn a_crashed_transcription_shows_an_error() {
    let mut h = subtitles_harness(std::sync::Arc::new(CrashingTranscriber));
    let before = h.state().history.project().clone();
    h.get_by_label("Generate").click();
    h.step();
    let start = std::time::Instant::now();
    while h
        .query_by_label("Couldn't generate subtitles: crashed")
        .is_none()
    {
        assert!(start.elapsed().as_secs() < 30, "never gave up");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    assert!(h.state().subtitles.is_open());
    assert_eq!(h.state().history.project(), &before);
}

/// Acceptance, end to end: Timeline > Transcript lists a cached fake transcript; clicking the
/// first word seeks to it, Shift+clicking the last picks the run, and Delete cuts it from the
/// clip as one undo step.
#[test]
fn transcript_panel_cuts_the_picked_words() {
    use jvid_core::history::Created;
    use jvid_core::model::{NewClip, TrackKind};
    use jvid_core::time::{FLICKS_PER_SECOND as S, Time};
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/samples/voice.m4a");
    jvid_media::ffmpeg::init().unwrap();
    let probe = jvid_media::probe::probe(&path).unwrap();
    let mut h = plain_harness();
    let app = h.state_mut();
    app.on_welcome = false;
    let duration = probe.duration;
    app.history
        .apply(Command::Import(vec![(path.clone(), probe)]))
        .unwrap();
    let (kind, name) = (TrackKind::Audio, String::new());
    let Ok(Created::Track(track)) = app.history.apply(Command::AddTrack { kind, name }) else {
        panic!("track")
    };
    let clip = NewClip {
        asset: app.history.project().assets()[0].id,
        source_in: Time(0),
        source_out: duration,
        start: Time(0),
        link: None,
    };
    app.history.apply(Command::AddClip { track, clip }).unwrap();
    let dir = std::env::temp_dir().join(format!("jvid-transcript-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    app.transcript.cache = crate::transcript::Cache::new(dir);
    // "one" at 0.2 s to "three" ending at 1.4 s.
    let at = |s: f64| Time((s * S as f64) as i64);
    let words = ["one", "two", "three"].iter().enumerate().map(|(i, w)| {
        let start = 0.2 + 0.4 * i as f64;
        jvid_transcribe::Word {
            text: (*w).into(),
            start: at(start),
            end: at(start + 0.4),
            confidence: 1.0,
        }
    });
    let source = jvid_transcribe::Source {
        path,
        nth: 0,
        range: 0..i64::MAX,
    };
    let heard = jvid_transcribe::Transcript {
        language: None,
        words: words.collect(),
    };
    app.transcript.cache.insert(&source, heard);
    h.run();
    h.get_by_label("Timeline").click();
    h.run();
    h.get_by_label("Transcript").click();
    h.run();
    let r = h.get_by_label("one two three").rect();
    let click = |h: &mut Harness<'static, App>, x: f32, modifiers| {
        let pos = egui::pos2(x, r.center().y);
        h.event(egui::Event::ModifiersChanged(modifiers));
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            });
            h.run();
        }
        h.event(egui::Event::ModifiersChanged(Default::default()));
    };
    click(&mut h, r.left() + 2.0, Default::default());
    assert_eq!(h.state().timeline.playhead(), at(0.2));
    click(&mut h, r.right() - 2.0, egui::Modifiers::SHIFT);
    h.key_press(egui::Key::Delete);
    h.run();
    let clips = |h: &Harness<'static, App>| {
        let seq = h.state().history.project().sequence();
        let c = |c: &jvid_core::model::Clip| (c.start, c.source_in, c.source_out);
        seq.tracks()[0].clips().iter().map(c).collect::<Vec<_>>()
    };
    let cut = vec![(Time(0), Time(0), at(0.2)), (at(0.2), at(1.4), duration)];
    assert_eq!(clips(&h), cut);
    assert!(h.query_by_label("one two three").is_none());
    h.state_mut().history.undo();
    h.run();
    assert_eq!(clips(&h), [(Time(0), Time(0), duration)]);
    assert!(h.query_by_label("one two three").is_some());

    // Find: Enter goes from match to match, and Backspace still edits the query, not the clip.
    h.state_mut().timeline.seek(Time(0));
    // The text field nearest the words: Transcript's Find, not the bin's search.
    let words = h.get_by_label("one two three").rect().center();
    let fields = h.get_all_by_role(egui::accesskit::Role::TextInput);
    let near = |n: &egui_kittest::Node<'_>| n.rect().center().distance(words) as i32;
    fields.min_by_key(near).unwrap().focus();
    h.run();
    h.event(egui::Event::Text("t".into()));
    h.run();
    for want in [at(0.6), at(1.0)] {
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().timeline.playhead(), want);
    }
    h.key_press(egui::Key::Backspace);
    h.run();
    assert_eq!(clips(&h), [(Time(0), Time(0), duration)]);
    assert_eq!(h.state().transcript.query, "");
}

/// An output with no device clock; lifecycle tests never touch host audio hardware.
struct LifecycleOutput;
impl jvid_audio::Backend for LifecycleOutput {
    fn devices(&self) -> Vec<String> {
        vec![]
    }
    fn open(
        &mut self,
        _: Option<&str>,
        _: std::sync::Arc<jvid_audio::Feed>,
    ) -> Result<(String, Box<dyn std::any::Any>), String> {
        Ok(("test".into(), Box::new(())))
    }
}

/// The lifecycle regressions exercise the real recorder without media decoders or a GPU.
fn recording_app(name: &str, armed: bool) -> (App, PathBuf, PathBuf, Time) {
    let dir = std::env::temp_dir().join(format!("jvid-lifecycle-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::restore(None);
    if armed {
        app.history
            .apply(Command::AddTrack {
                kind: TrackKind::Audio,
                name: String::new(),
            })
            .unwrap();
        app.timeline.armed = app
            .history
            .project()
            .sequence()
            .tracks()
            .last()
            .map(|t| t.id);
    }
    assert!(app.save_to(dir.join("old.jvid")));
    let start = Time(2 * jvid_core::time::FLICKS_PER_SECOND);
    app.timeline.action("selection_tool", app.history.project());
    app.timeline.seek(start);
    assert_eq!(app.timeline.playhead(), start);
    let mic = crate::record::tests::FakeMic::default();
    app.input = Some(Box::new(mic.clone()));
    app.preview
        .set_output(jvid_audio::Output::new(Box::new(LifecycleOutput), None));
    app.start_take();
    let wav = app.take.as_ref().unwrap().path.clone();
    mic.0.lock().unwrap().as_ref().unwrap().push(&[0.25; 4_800]);
    (app, dir, wav, start)
}

fn assert_finished_take(project: &Project, wav: &Path, start: Time) {
    let bytes = std::fs::read(wav).unwrap();
    assert_eq!(bytes.len(), 44 + 4_800 * 2);
    assert_eq!(
        u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
        4_800 * 2
    );
    let asset = project.assets().iter().find(|a| a.path == wav).unwrap();
    let clip = project
        .sequence()
        .tracks()
        .iter()
        .flat_map(|t| t.clips())
        .find(|c| c.asset == asset.id)
        .unwrap();
    assert_eq!(clip.start, start);
    assert_eq!(
        clip.end() - clip.start,
        Time(jvid_core::time::FLICKS_PER_SECOND / 10)
    );
}

#[test]
fn audit_433_close_cancel_finalizes_clean_armed_take_and_keeps_one_undo_edit() {
    let (mut app, dir, wav, start) = recording_app("cancel", true);
    assert!(!app.history.is_dirty());
    let before = app.history.project().clone();
    app.discard_then(Then::Close);
    assert_eq!(app.prompts, [Prompt::Discard(Then::Close)]);
    assert!(!app.closing && app.take.is_none());
    let prompt = app.prompts.remove(0);
    app.answer(prompt, 1);
    assert!(app.history.is_dirty() && !app.closing);
    assert!(
        !app.timeline
            .recorded_waveform(&wav)
            .unwrap()
            .finest()
            .is_empty()
    );
    assert_finished_take(app.history.project(), &wav, start);
    assert!(app.history.undo());
    assert_eq!(app.history.project(), &before);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_433_window_close_finalizes_before_offering_save() {
    let (app, dir, wav, start) = recording_app("window-close", true);
    let mut h = builder([1280.0, 720.0], 1.0).build_eframe(move |cc| {
        theme::install(&cc.egui_ctx);
        app
    });
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    h.run_steps(1);
    assert_eq!(h.state().prompts, [Prompt::Discard(Then::Close)]);
    assert!(h.state().take.is_none() && !h.state().closing);
    assert_finished_take(h.state().history.project(), &wav, start);
    let state = h.state_mut();
    let prompt = state.prompts.remove(0);
    state.answer(prompt, 0);
    assert!(h.state().closing && !h.state().history.is_dirty());
    assert_finished_take(
        &project_file::load(&dir.join("old.jvid")).unwrap(),
        &wav,
        start,
    );
    drop(h);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_433_save_before_navigation_includes_complete_take() {
    for name in ["new", "open", "close"] {
        let (mut app, dir, wav, start) = recording_app(name, name != "new");
        let then = match name {
            "new" => Then::New,
            "open" => {
                let next = dir.join("next.jvid");
                project_file::save(&Project::default(), &next).unwrap();
                Then::Recent(next)
            }
            _ => Then::Close,
        };
        app.discard_then(then);
        let prompt = app.prompts.remove(0);
        app.answer(prompt, 0);
        let saved = project_file::load(&dir.join("old.jvid")).unwrap();
        assert_finished_take(&saved, &wav, start);
        assert!(app.take.is_none());
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn audit_433_completed_recording_is_one_undo_edit_after_track_creation() {
    let (mut app, dir, wav, start) = recording_app("new-track", false);
    let before_finish = app.history.project().clone();
    app.discard_then(Then::Close);
    let prompt = app.prompts.remove(0);
    app.answer(prompt, 1);
    assert_finished_take(app.history.project(), &wav, start);
    assert!(app.history.undo());
    assert_eq!(app.history.project(), &before_finish);
    assert!(
        app.history.is_dirty(),
        "the new empty track remains its own edit"
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_433_failed_finalization_blocks_navigation_and_keeps_finalized_wav() {
    let (mut app, dir, wav, _) = recording_app("failed-finalization", true);
    app.take.as_mut().unwrap().track = jvid_core::model::TrackId(999);
    let before = app.history.project().clone();
    app.discard_then(Then::Close);
    assert!(!app.closing);
    assert!(matches!(app.prompts.as_slice(), [Prompt::Error(_)]));
    assert_eq!(app.history.project(), &before);
    let bytes = std::fs::read(wav).unwrap();
    assert_eq!(
        u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
        4_800 * 2
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

fn unrelated_edit_during_take(armed: bool, fail: bool) {
    let name = format!("independent-{armed}-{fail}");
    let (mut app, dir, _, _) = recording_app(&name, armed);
    let before_edit = app.history.project().clone();
    app.history
        .apply(Command::SetSettings(ProjectSettings {
            width: 1280,
            ..Default::default()
        }))
        .unwrap();
    let after_edit = app.history.project().clone();
    if fail {
        app.take.as_mut().unwrap().track = jvid_core::model::TrackId(999);
    }
    app.discard_then(Then::Close);
    assert!(!app.closing);
    if fail {
        assert!(matches!(app.prompts.as_slice(), [Prompt::Error(_)]));
        assert_eq!(
            app.history.project(),
            &after_edit,
            "failed finish rolled back an unrelated edit"
        );
    } else {
        let prompt = app.prompts.remove(0);
        app.answer(prompt, 1);
        assert!(app.history.undo(), "one completed recording edit");
        assert_eq!(
            app.history.project(),
            &after_edit,
            "recording undo absorbed an unrelated edit"
        );
    }
    assert!(
        app.history.undo(),
        "the independent edit retains its own undo step"
    );
    assert_eq!(app.history.project(), &before_edit);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_433_recording_undo_preserves_intervening_edits() {
    for armed in [false, true] {
        unrelated_edit_during_take(armed, false);
    }
}

#[test]
fn audit_433_failed_finish_preserves_intervening_edits() {
    for armed in [false, true] {
        unrelated_edit_during_take(armed, true);
    }
}

#[test]
fn audit_437_open_with_reused_ids_clears_selection_clipboard_and_source() {
    let dir = std::env::temp_dir().join(format!("jvid-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::restore(None);
    crate::playback::tests::add_solid(&mut app.history, [1.0, 0.0, 0.0, 1.0], Time(0));
    let project_a = app.history.project().clone();
    let asset = project_a.assets()[0].id;
    let clip = project_a
        .sequence()
        .tracks()
        .iter()
        .flat_map(|t| t.clips())
        .next()
        .unwrap()
        .id;
    let path_b = dir.join("b.jvid");
    let mut project_b = History::new(Project::default());
    crate::playback::tests::add_solid(&mut project_b, [0.0, 0.0, 1.0, 1.0], Time(0));
    assert_eq!(project_b.project().assets()[0].id, asset);
    assert!(project_b.project().sequence().clip(clip).is_some());
    project_file::save(project_b.project(), &path_b).unwrap();
    app.timeline.action("select_all", &project_a);
    app.timeline.action("copy", &project_a);
    app.preview.open(&project_a, asset);
    app.bin.selected = Some(asset);
    app.bin.renaming = Some((asset, "stale".into()));
    app.bin.confirm_remove = Some(asset);
    app.settings_dialog.draft = Some(project_a.settings().clone());
    app.transcript.query = "project A".into();
    app.timeline.action("mark_in", &project_a);
    app.open(path_b);
    assert!(app.timeline.selected().is_empty());
    assert_eq!(app.preview.source_asset(), None);
    assert!(
        app.preview
            .source_action("mark_in", app.history.project())
            .is_none()
    );
    assert!(
        app.bin.selected.is_none()
            && app.bin.renaming.is_none()
            && app.bin.confirm_remove.is_none()
    );
    assert!(app.transcript.query.is_empty());
    assert!(app.settings_dialog.draft.is_none());
    let before = app.history.project().clone();
    app.on_welcome = false;
    let mut h = builder([1280.0, 720.0], 1.0).build_eframe(move |cc| {
        theme::install(&cc.egui_ctx);
        app
    });
    h.run_steps(2);
    assert_eq!(h.state().history.project(), &before, "stale deferred edit");
    h.state_mut().run_action("delete", true);
    h.state_mut().run_action("paste", true);
    h.run_steps(2);
    assert_eq!(
        h.state().history.project(),
        &before,
        "stale selection or clipboard"
    );
    let app = h.state_mut();
    // A failed Open retains the current editing session and selection.
    app.timeline.select_clip(before.sequence(), clip);
    app.open(dir.join("missing.jvid"));
    assert!(app.timeline.selected().contains(&clip));
    assert_eq!(app.history.project(), &before);
    drop(h);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_437_late_probe_cannot_mutate_replacement_sessions() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    static PHASE: AtomicUsize = AtomicUsize::new(0);
    fn delayed(_: &Path) -> Result<jvid_core::model::ProbeInfo, String> {
        PHASE.store(1, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(5);
        while PHASE.load(Ordering::SeqCst) == 1 {
            assert!(Instant::now() < until, "test never released the probe");
            std::thread::sleep(Duration::from_millis(1));
        }
        PHASE.store(3, Ordering::SeqCst);
        Ok(jvid_core::model::ProbeInfo {
            width: Some(640),
            height: Some(480),
            ..Default::default()
        })
    }
    let dir = std::env::temp_dir().join(format!("jvid-delayed-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("b.jvid");
    project_file::save(&Project::default(), &path).unwrap();
    for replacement in ["new", "open", "recovery", "tutorial"] {
        PHASE.store(0, Ordering::SeqCst);
        let mut app = App::restore(None);
        app.importer = Importer::new(jvid_jobs::Pool::new(1), delayed);
        app.import([dir.join("delayed.mkv")]);
        let until = Instant::now() + Duration::from_secs(5);
        while PHASE.load(Ordering::SeqCst) != 1 {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        let began = Instant::now();
        match replacement {
            "new" => app.new_project(None),
            "open" => app.open(path.clone()),
            "recovery" => app.answer(
                Prompt::Restore {
                    path: Some(path.clone()),
                    auto: path.clone(),
                },
                0,
            ),
            "tutorial" => {
                app.start_lesson(&tutorial::LESSONS[0]);
                app.end_lesson();
            }
            _ => unreachable!(),
        }
        // Release before assertions so failure cannot leave a worker waiting on the test.
        PHASE.store(2, Ordering::SeqCst);
        assert!(
            began.elapsed() < Duration::from_secs(1),
            "switch waited for an abandoned probe"
        );
        while PHASE.load(Ordering::SeqCst) != 3 {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        // The producer has signalled and the job has returned before checking the receiver.
        let until = Instant::now() + Duration::from_secs(5);
        while !app.importer.jobs_idle() {
            assert!(Instant::now() < until, "probe worker did not finish");
            std::thread::yield_now();
        }
        app.poll_import(0.0);
        assert!(
            app.history.project().assets().is_empty(),
            "stale import after {replacement}"
        );
        assert_eq!(
            app.history.project().settings().width,
            ProjectSettings::default().width
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_437_new_clears_canvas_text_and_pending_region_request() {
    use jvid_core::history::Created;
    use jvid_core::model::{NewClip, Text};
    let mut h = plain_harness();
    let app = h.state_mut();
    let Created::Asset(asset) = app
        .history
        .apply(Command::AddText {
            name: "A".into(),
            text: Text::default(),
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = app
        .history
        .apply(Command::AddTrack {
            kind: TrackKind::Video,
            name: "V1".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = app
        .history
        .apply(Command::AddClip {
            track,
            clip: NewClip {
                asset,
                source_in: Time(0),
                source_out: Time(jvid_core::time::FLICKS_PER_SECOND),
                start: Time(0),
                link: None,
            },
        })
        .unwrap()
    else {
        panic!("clip")
    };
    crate::handles::start_editing(&h.ctx, h.state().history.project(), clip, egui::Pos2::ZERO);
    crate::hide_region::request(&h.ctx, clip);
    assert!(crate::handles::editing(&h.ctx));
    h.state_mut()
        .open(PathBuf::from("/nonexistent/jvid-session-failure.jvid"));
    assert!(
        crate::handles::editing(&h.ctx),
        "failed Open preserves the editor session"
    );
    h.state_mut().new_project(None);
    assert!(!crate::handles::editing(&h.ctx));
    assert_eq!(crate::hide_region::requested(&h.ctx), None);
}

#[test]
fn audit_461_named_recovery_discard_survives_reopen_and_preserves_other_projects() {
    let dir = std::env::temp_dir().join(format!("jvid-recovery-discard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("named.jvid");
    let other = dir.join("other.jvid");
    project_file::save(&Project::default(), &path).unwrap();
    let main_bytes = std::fs::read(&path).unwrap();
    let mut edited = History::new(Project::default());
    edited
        .apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: "recovered".into(),
        })
        .unwrap();
    // Explicit timestamps avoid relying on filesystem timestamp resolution.
    let main = std::fs::File::open(&path).unwrap();
    main.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    autosave::write(edited.project(), &path).unwrap();
    autosave::write(edited.project(), &other).unwrap();
    let other_bytes = std::fs::read(autosave::slot(&other, 1)).unwrap();
    let mut app = App::restore(None);
    app.open(path.clone());
    let prompt = app.prompts.remove(0);
    assert!(matches!(prompt, Prompt::Restore { .. }));
    app.answer(prompt, 1);
    assert!(!app.history.is_dirty());
    app.discard_then(Then::Close);
    assert!(app.closing);
    drop(app);
    let mut restarted = App::restore(None);
    restarted.open(path.clone());
    assert!(restarted.prompts.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), main_bytes);
    assert_eq!(
        std::fs::read(autosave::slot(&other, 1)).unwrap(),
        other_bytes
    );
    drop(restarted);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_461_restore_success_is_dirty_and_failure_preserves_recovery() {
    let dir = std::env::temp_dir().join(format!("jvid-recovery-restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("named.jvid");
    project_file::save(&Project::default(), &path).unwrap();
    let mut edited = History::new(Project::default());
    edited
        .apply(Command::AddTrack {
            kind: TrackKind::Audio,
            name: "restored".into(),
        })
        .unwrap();
    autosave::write(edited.project(), &path).unwrap();
    let auto = autosave::slot(&path, 1);
    let mut app = App::restore(None);
    app.open(path.clone());
    app.prompts.clear();
    app.answer(
        Prompt::Restore {
            path: Some(path.clone()),
            auto: auto.clone(),
        },
        0,
    );
    assert_eq!(app.history.project(), edited.project());
    assert!(app.history.is_dirty());
    assert!(auto.exists());
    std::fs::write(&auto, "broken recovery").unwrap();
    let before = app.history.project().clone();
    app.answer(
        Prompt::Restore {
            path: Some(path),
            auto: auto.clone(),
        },
        0,
    );
    assert_eq!(app.history.project(), &before);
    assert_eq!(std::fs::read_to_string(&auto).unwrap(), "broken recovery");
    assert!(matches!(app.prompts.last(), Some(Prompt::Error(_))));
    let untitled = dir.join("Untitled.jvid");
    autosave::write(edited.project(), &untitled).unwrap();
    app.untitled = Some(untitled.clone());
    app.answer(
        Prompt::Restore {
            path: None,
            auto: autosave::slot(&untitled, 1),
        },
        1,
    );
    assert!(!autosave::slot(&untitled, 1).exists());
    autosave::write(edited.project(), &untitled).unwrap();
    let mut fresh = App::restore(None);
    fresh.untitled = Some(untitled.clone());
    fresh.answer(
        Prompt::Restore {
            path: None,
            auto: autosave::newer(&untitled).unwrap(),
        },
        0,
    );
    assert_eq!(fresh.path, None);
    assert_eq!(fresh.history.project(), edited.project());
    assert!(fresh.history.is_dirty() && !fresh.on_welcome);
    assert!(autosave::slot(&untitled, 1).exists());
    drop(fresh);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
