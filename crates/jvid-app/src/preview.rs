//! The Preview panel's viewer: the current source's frame, decoded off-thread and drawn by
//! jvid-render through an egui paint callback, fitted or zoomed on Viewer black.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, wgpu};
use egui::{Align2, Rect, Ui, UiBuilder, vec2};
use jvid_core::settings::PreviewQuality;
use jvid_core::time::Time;
use jvid_jobs::Priority;
use jvid_media::decode::{DecoderPool, Frame};
use jvid_render::preview::{self as render, Yuv};
use suite_ui::theme;
use suite_ui::tokens::{color, size, space};
use suite_ui::widgets::Dropdown;

const ZOOMS: [(&str, Option<f32>); 4] = [
    ("Fit", None),
    ("50%", Some(0.5)),
    ("100%", Some(1.0)),
    ("200%", Some(2.0)),
];
const QUALITIES: [(&str, PreviewQuality); 3] = [
    ("Full", PreviewQuality::Full),
    ("½", PreviewQuality::Half),
    ("¼", PreviewQuality::Quarter),
];

type Decoded = (PathBuf, Result<Frame, String>);

pub struct Preview {
    pool: Arc<DecoderPool>,
    jobs: jvid_jobs::Pool,
    tx: Sender<Decoded>,
    rx: Receiver<Decoded>,
    /// The source last asked for, and what came back for it.
    source: Option<PathBuf>,
    frame: Option<Arc<Frame>>,
    error: Option<String>,
    /// Bumped when `frame` or the preview scale changes; the GPU side re-uploads when it sees a new one.
    generation: u64,
    zoom: usize,
    quality: usize,
}

impl Preview {
    /// Puts the GPU half in egui-wgpu's callback resources; without wgpu the viewer stays empty.
    pub fn new(render_state: Option<&egui_wgpu::RenderState>) -> Self {
        if let Some(rs) = render_state {
            let preview = render::Preview::new(&rs.device, rs.target_format);
            let gpu = Gpu {
                preview,
                shown: None,
            };
            rs.renderer.write().callback_resources.insert(gpu);
        }
        let (tx, rx) = channel();
        Self {
            pool: Arc::default(),
            jobs: jvid_jobs::Pool::new(1),
            tx,
            rx,
            source: None,
            frame: None,
            error: None,
            generation: 0,
            zoom: 0,
            quality: 0,
        }
    }

    /// Shows `source`'s first frame, decoding it first if it changed; an `offline` source
    /// shows a placeholder instead.
    // shortcut: always the first frame; the playhead and Source/Program split arrive with #24 and #25.
    pub fn show(&mut self, ui: &mut Ui, source: Option<&Path>, offline: bool) {
        if self.source.as_deref() != source || (offline && self.error.is_none()) {
            self.source = source.map(Path::to_path_buf);
            (self.frame, self.error) = (None, offline.then(|| "Media offline".to_owned()));
            if let Some(path) = source.filter(|_| !offline) {
                let (tx, ctx, p) = (self.tx.clone(), ui.ctx().clone(), path.to_path_buf());
                self.pool.submit(
                    &self.jobs,
                    path.to_path_buf(),
                    Time(0),
                    Priority::Interactive,
                    move |r| {
                        let _ = tx.send((p, r.map_err(|e| e.to_string())));
                        ctx.request_repaint();
                    },
                );
            }
        }
        while let Ok((path, r)) = self.rx.try_recv() {
            if self.source.as_deref() != Some(&path) {
                continue;
            }
            match r {
                Ok(f) => {
                    self.frame = Some(Arc::new(f));
                    self.generation += 1;
                }
                Err(e) => {
                    tracing::warn!("preview decode of {} failed: {e}", path.display());
                    self.error = Some(e);
                }
            }
        }
        if self.source.is_none() {
            return;
        }

        let rect = ui.max_rect();
        let viewer = Rect::from_min_max(rect.min, rect.max - vec2(0.0, size::HEADER));
        ui.painter().rect_filled(viewer, 0.0, color::VIEWER);
        if offline {
            ui.painter().text(
                viewer.center(),
                Align2::CENTER_BOTTOM,
                theme::icon::CIRCLE_ALERT,
                theme::icon_font(),
                color::RED,
            );
            ui.painter().text(
                viewer.center() + vec2(0.0, space::S8),
                Align2::CENTER_TOP,
                "Media offline. Relink it from the media bin.",
                theme::body(),
                color::INK_2,
            );
        } else if let Some(e) = &self.error {
            ui.painter().text(
                viewer.center(),
                Align2::CENTER_CENTER,
                format!("Can't show this frame: {e}"),
                theme::body(),
                color::INK_2,
            );
        }
        if let Some(f) = &self.frame {
            let view = [viewer.left(), viewer.top(), viewer.width(), viewer.height()];
            // 100% is one frame pixel per screen pixel.
            let zoom = ZOOMS[self.zoom].1.map(|z| z / ui.pixels_per_point());
            let image = render::place([f.width as f32, f.height as f32], view, zoom);
            let paint = Paint {
                frame: f.clone(),
                generation: self.generation,
                scale: match QUALITIES[self.quality].1 {
                    PreviewQuality::Full => 1,
                    PreviewQuality::Half => 2,
                    PreviewQuality::Quarter => 4,
                },
                clip: render::to_clip(image, view),
            };
            ui.painter()
                .add(egui_wgpu::Callback::new_paint_callback(viewer, paint));
        }

        let row = Rect::from_x_y_ranges(rect.x_range(), viewer.bottom()..=rect.bottom());
        ui.scope_builder(UiBuilder::new().max_rect(row), |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = space::S8;
                let zooms = ZOOMS.map(|z| z.0);
                ui.add(Dropdown::new(&mut self.zoom, &zooms).width(88.0))
                    .on_hover_text("Zoom");
                let qualities = QUALITIES.map(|q| q.0);
                if ui
                    .add(Dropdown::new(&mut self.quality, &qualities).width(72.0))
                    .on_hover_text("Preview resolution")
                    .changed()
                {
                    self.generation += 1;
                }
            });
        });
    }
}

/// The GPU half, kept in egui-wgpu's callback resources.
struct Gpu {
    preview: render::Preview,
    /// The generation of the uploaded image.
    shown: Option<u64>,
}

/// One frame's draw. Uploads (and converts) only when the frame or preview scale changed.
struct Paint {
    frame: Arc<Frame>,
    generation: u64,
    scale: u32,
    /// The image rect in the callback viewport's clip space.
    clip: [f32; 4],
}

impl CallbackTrait for Paint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get_mut::<Gpu>() {
            if gpu.shown != Some(self.generation) {
                gpu.shown = Some(self.generation);
                let f = &self.frame;
                let yuv = Yuv {
                    width: f.width,
                    height: f.height,
                    planes: [&f.planes[0], &f.planes[1], &f.planes[2]],
                    bt709: f.bt709,
                    full_range: f.full_range,
                };
                gpu.preview.upload(device, queue, &yuv, self.scale);
            }
            gpu.preview.set_dst(queue, self.clip);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(gpu) = resources.get::<Gpu>() {
            gpu.preview.paint(pass);
        }
    }
}

#[cfg(test)]
impl Preview {
    pub fn has_frame(&self) -> bool {
        self.frame.is_some()
    }

    /// The decode for the current source has come back, with a frame or an error.
    pub fn settled(&self) -> bool {
        self.frame.is_some() || self.error.is_some()
    }
}
