//! The confirm dialog: a modal over a dimmed backdrop with a title, a body and
//! right-aligned buttons.

use egui::{Align, Context, Frame, Id, Layout, RichText};

use crate::theme;
use crate::tokens::{radius, space};
use crate::widgets::{Button, ButtonKind};

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
        let modal = egui::Modal::new(Id::new("suite_ui_dialog"))
            .frame(
                Frame::popup(&ctx.global_style())
                    .corner_radius(radius::DIALOG)
                    .inner_margin(space::S16),
            )
            .show(ctx, |ui| {
                ui.set_width(if self.long { 640.0 } else { 360.0 });
                ui.label(RichText::new(self.title).font(theme::headline()));
                ui.add_space(space::S8);
                if self.long {
                    egui::ScrollArea::vertical()
                        .max_height(ctx.content_rect().height() * 0.6)
                        .show(ui, |ui| ui.label(self.body));
                } else {
                    ui.label(self.body);
                }
                ui.add_space(space::S16);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = space::S8;
                    for (i, &(label, kind)) in self.buttons.iter().enumerate() {
                        let r = ui.add(Button::new(kind, label));
                        if i == 0 && ui.memory(|m| m.focused().is_none()) {
                            r.request_focus();
                        }
                        if r.clicked() {
                            picked = Some(i);
                        }
                    }
                });
            });
        if picked.is_none() && modal.should_close() {
            picked = Some(self.cancel);
        }
        picked
    }
}
