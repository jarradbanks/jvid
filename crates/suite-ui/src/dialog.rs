//! The confirm dialog: a modal over a dimmed backdrop with a title, a body and
//! right-aligned buttons.

use egui::{Align, Context, Frame, Id, Layout, Response, RichText, Ui};

use crate::theme;
use crate::tokens::{radius, space};
use crate::widgets::{Button, ButtonKind};

/// Content-sized, right-aligned dialog or popup actions, in reading and Tab order:
/// cancel/secondary actions first, primary last. Rows wrap at narrow widths, keeping
/// the primary action at the bottom right. Responses retain the input order.
///
/// Call after the body (and any body scroll area); this never claims spare height.
pub fn action_footer<'a>(
    ui: &mut Ui,
    buttons: impl IntoIterator<Item = Button<'a>>,
) -> Vec<Response> {
    let id = ui.make_persistent_id("action_footer");
    let width = ui.available_width().max(1.0);
    let buttons: Vec<_> = buttons
        .into_iter()
        .map(|b| b.height(28.0).wrap_at(width))
        .collect();
    let sizes: Vec<_> = buttons.iter().map(|b| b.desired_size(ui)).collect();
    let mut rows = Vec::new();
    let mut start = 0;
    let mut row_width = 0.0;
    for (i, size) in sizes.iter().enumerate() {
        let next = if i == start {
            size.x
        } else {
            row_width + space::S8 + size.x
        };
        if next > width && i > start {
            rows.push((start..i, row_width));
            start = i;
            row_width = size.x;
        } else {
            row_width = next;
        }
    }
    if start < buttons.len() {
        rows.push((start..buttons.len(), row_width));
    }
    let mut buttons = buttons.into_iter();
    ui.allocate_ui_with_layout(egui::vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing = egui::Vec2::splat(space::S8);
        let mut responses = Vec::with_capacity(sizes.len());
        for (row, row_width) in rows {
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add_space((width - row_width).max(0.0));
                    for i in row {
                        let button = buttons.next().expect("footer button");
                        // Resizing may move an action to another row; keep keyboard focus.
                        responses.push(
                            ui.scope_builder(egui::UiBuilder::new().id(id.with(i)), |ui| {
                                if ui.is_sizing_pass() {
                                    ui.allocate_exact_size(sizes[i], egui::Sense::hover()).1
                                } else {
                                    ui.add(button)
                                }
                            })
                            .inner,
                        );
                    }
                },
            );
        }
        responses
    })
    .inner
}

/// A confirm dialog. `buttons[0]` is the default: drawn rightmost and focused, so Enter
/// picks it; Tab moves between buttons without leaving the dialog. Esc or a backdrop
/// click picks `cancel`.
pub struct Dialog<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub buttons: &'a [(&'a str, ButtonKind)],
    pub cancel: usize,
    /// A long body, such as licence text: the dialog is wider and the body scrolls.
    pub long: bool,
}

impl Dialog<'_> {
    /// Draws the dialog; the index of the button picked this frame, if any.
    pub fn show(&self, ctx: &Context) -> Option<usize> {
        let mut picked = None;
        let id = Id::new("suite_ui_dialog");
        let previous_rect = ctx.memory(|m| m.area_rect(id));
        let modal = egui::Modal::new(id)
            .frame(
                Frame::popup(&ctx.global_style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ctx, |ui| {
                ui.set_width(
                    (if self.long { 640.0_f32 } else { 360.0_f32 })
                        .min((ctx.content_rect().width() - 48.0).max(1.0)),
                );
                ui.label(RichText::new(self.title).font(theme::headline()));
                ui.add_space(space::S8);
                if self.long {
                    let height = ctx.content_rect().height() * 0.6;
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(height)
                        .max_height(height)
                        .show(ui, |ui| ui.label(self.body));
                } else {
                    ui.label(self.body);
                }
                ui.add_space(space::S16);
                let buttons = self
                    .buttons
                    .iter()
                    .rev()
                    .map(|&(label, kind)| Button::new(kind, label));
                let responses = action_footer(ui, buttons);
                for (i, r) in responses.into_iter().rev().enumerate() {
                    if i == 0 && !ui.is_sizing_pass() && ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                    if r.clicked() {
                        picked = Some(i);
                    }
                }
            });
        // The shared Area centres using its previous size. Repaint after a content
        // change so the footer reaches its final position before the next input.
        if previous_rect.is_some_and(|r| r.size() != modal.response.rect.size()) {
            ctx.request_repaint();
        }
        if picked.is_none() && modal.should_close() {
            picked = Some(self.cancel);
        }
        picked
    }
}
