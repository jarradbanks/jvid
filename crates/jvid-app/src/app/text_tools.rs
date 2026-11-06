//! Text presets and the Program text tool share one atomic, non-destructive placement.

use egui::{Pos2, Rect, Sense, Ui};
use jvid_core::history::Created;
use jvid_core::model::{ClipId, Placement, Text};
use jvid_core::time::FLICKS_PER_SECOND;

use super::*;

pub(super) struct Creation {
    pub name: String,
    pub text: Text,
    pub at: Option<Pos2>,
}

/// Only the project picture accepts text placement; letterboxing and transport stay separate.
pub(super) fn canvas(ui: &mut Ui, frame: Rect) -> Option<Creation> {
    if egui::DragAndDrop::has_any_payload(ui.ctx()) {
        return None;
    }
    let geometry = crate::viewer_geometry::ViewerGeometry::from_frame(
        crate::preview::viewer_rect(ui),
        frame,
        frame.size(),
    );
    let hit = geometry.picture();
    let r = ui.interact(hit, ui.id().with("text tool"), Sense::click());
    r.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr!("action.text_tool"))
    });
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    let at = r.interact_pointer_pos().filter(|_| r.clicked())?;
    let (name, _, mut text) = crate::media_bin::presets().into_iter().next()?;
    let point = geometry.screen_to_normalized(at);
    text.anchor = [point.x.clamp(0.0, 1.0), point.y.clamp(0.0, 1.0)];
    Some(Creation {
        name: name.into(),
        text,
        at: Some(at),
    })
}

impl App {
    /// Rehearse identity allocation on a copy so asset, optional track and clip are one
    /// history step. Never overwrite footage or put the title under another visible layer.
    pub(super) fn create_text(
        &mut self,
        ctx: &egui::Context,
        creation: Creation,
    ) -> Result<ClipId, ModelError> {
        let add = Command::AddText {
            name: creation.name,
            text: creation.text,
        };
        let mut trial = History::new(self.history.project().clone());
        let Created::Asset(asset) = trial.apply(add.clone())? else {
            unreachable!()
        };
        let head = self.timeline.playhead();
        // Generic source placement refuses an all-locked video stack. Text creation can
        // always add an overlay, so start with a fresh video-only placement instead.
        let mut place = Placement {
            asset,
            source_in: Time(0),
            source_out: Time(
                i64::from(jvid_core::settings::DEFAULT_STILL_SECS) * FLICKS_PER_SECOND,
            ),
            start: head,
            video: None,
            audio: None,
            streams: Vec::new(),
            add_video: true,
            add_audio: false,
            insert: false,
        };
        let end = head + place.source_out - place.source_in;
        let seq = trial.project().sequence();
        let solo = seq
            .tracks()
            .iter()
            .any(|t| t.kind == TrackKind::Video && t.solo);
        let top = seq
            .tracks()
            .iter()
            .find(|t| t.kind == TrackKind::Video && !t.hidden && (!solo || t.solo));
        place.video = top
            .filter(|t| {
                !t.locked
                    && !t.subtitles
                    && t.clips().iter().all(|c| c.end() <= head || c.start >= end)
            })
            .map(|t| t.id);
        place.add_video = place.video.is_none();
        let put = Command::Place(place);
        let Created::Clip(clip) = trial.apply(put.clone())? else {
            unreachable!()
        };
        let mut commands = vec![add, put];
        // A new overlay must participate in the current video solo mix to be visible.
        if solo {
            let (track, _) = trial.project().sequence().clip(clip).expect("placed title");
            let mut settings = trial
                .project()
                .sequence()
                .track(track)
                .expect("title track")
                .settings();
            if !settings.solo {
                settings.solo = true;
                commands.push(Command::SetTrack(settings));
            }
        }
        self.history.apply(Command::Batch(commands))?;
        self.timeline.select_program_clip(clip);
        self.bin.selected = Some(asset);
        crate::inspector::choose(ctx, crate::inspector::Pane::Text);
        if let Some(at) = creation.at {
            crate::handles::start_editing(ctx, self.history.project(), clip, at);
        }
        ctx.request_repaint();
        Ok(clip)
    }
}
