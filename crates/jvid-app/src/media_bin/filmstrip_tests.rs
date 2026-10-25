//! Access the real bin paint consumer from the cross-panel regressions.
use super::*;

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
