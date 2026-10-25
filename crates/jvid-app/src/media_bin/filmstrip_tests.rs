//! Access the real bin paint consumer from the cross-panel regressions.
use super::*;

impl MediaBin {
    pub(crate) fn filmstrip_test_state(&self) -> (usize, u64, bool) {
        let cache = self.thumbs.cache.as_ref().unwrap();
        (
            self.thumbs.strip_requests.pending(),
            cache.filmstrips.requests_started(),
            cache.jobs.is_idle(),
        )
    }
}

pub(crate) struct BinConsumer(Thumbs);

impl BinConsumer {
    pub(crate) fn new(cache: Arc<Caches>) -> Self {
        Self(Thumbs {
            cache: Some(cache),
            ..Default::default()
        })
    }

    pub(crate) fn paint(&mut self, ui: &Ui, asset: &MediaAsset) {
        self.0.paint(
            ui,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 90.0)),
            asset,
            false,
            Some(0.5),
        );
    }

    pub(crate) fn poster(&mut self, ui: &Ui, asset: &MediaAsset) {
        self.0.paint(
            ui,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 90.0)),
            asset,
            false,
            None,
        );
    }

    pub(crate) fn on_focus(&mut self, ctx: &egui::Context) {
        self.0.strip_requests.on_focus(ctx, self.0.cache.as_deref());
    }

    pub(crate) fn texture(&self, path: &std::path::Path) -> egui::TextureId {
        self.0.strips[path].as_ref().unwrap()[0].id()
    }

    pub(crate) fn frames(&self, path: &std::path::Path) -> usize {
        self.0
            .strips
            .get(path)
            .and_then(Option::as_ref)
            .map_or(0, Vec::len)
    }

    pub(crate) fn pending(&self) -> usize {
        self.0.strip_requests.pending()
    }

    pub(crate) fn contains(&self, path: &std::path::Path) -> bool {
        self.0.strips.contains_key(path)
    }
}

#[test]
fn failed_hover_drains_completion_repaints_before_retrying_on_later_activity() {
    use std::time::{Duration, Instant};

    let dir = std::env::temp_dir().join(format!("jvid-failed-hover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut caches = Caches::new(&jvid_core::settings::Settings {
        cache_dir: Some(dir.clone()),
        ..Default::default()
    });
    Arc::get_mut(&mut caches).unwrap().thumbs = Arc::new(Cache::with_grab(
        dir.join("fake"),
        u64::MAX,
        Box::new(|_, _, _| Err("injected decode failure".into())),
    ));
    let mut bin = BinConsumer::new(caches.clone());
    let asset = MediaAsset {
        id: AssetId(1),
        path: dir.join("missing.mov"),
        probe: Some(jvid_core::model::ProbeInfo {
            duration: jvid_core::time::Time(3 * FLICKS_PER_SECOND),
            width: Some(160),
            height: Some(90),
            ..Default::default()
        }),
        name: None,
        in_point: None,
        out_point: None,
        solid: None,
        text: None,
        sequence: None,
    };
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let mut paint = |events| {
        // Every completion pass crosses the backoff, as on a slow/busy UI thread.
        time += 2.0 * filmstrips::RETRY_DELAY.as_secs_f64();
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| bin.paint(ui, &asset),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !caches.jobs.is_idle() {
            assert!(Instant::now() < deadline, "hover decode never completed");
            std::thread::sleep(Duration::from_millis(1));
        }
        let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        output.textures_delta.clear();
        delay
    };
    for attempt in 1..=2 {
        let mut delay = paint(vec![egui::Event::PointerMoved(pos2(80.0, 45.0))]);
        for _ in 0..8 {
            assert_eq!(caches.filmstrips.requests_started(), attempt);
            if delay == Duration::MAX {
                break;
            }
            delay = paint(Vec::new());
        }
        assert_eq!(
            delay,
            Duration::MAX,
            "failed hover kept scheduling repaints"
        );
        assert_eq!(caches.filmstrips.requests_started(), attempt);
        assert_eq!(caches.filmstrips.retained(), (0, 0));
    }
    assert_eq!(bin.pending(), 0);
    assert!(!bin.contains(&asset.path));
    let _ = std::fs::remove_dir_all(dir);
}
