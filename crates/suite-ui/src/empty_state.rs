//! Quiet, shared guidance for an empty panel body. Headers and footers belong to the caller.

use egui::{Align, Color32, Label, Rect, Response, Sense, Ui, pos2, vec2};

use crate::theme;
use crate::tokens::{color, space};
use crate::widgets::Button;

/// An unframed outline icon, a 14px heading, 12px help, and an optional existing action.
/// Use only for empty content or no selection; loading and errors keep their own treatment.
pub struct EmptyState<'a> {
    icon: &'a str,
    title: &'a str,
    help: &'a str,
    action: Option<(&'a str, bool)>,
    viewer: bool,
}

pub struct EmptyStateResponse {
    pub action: Option<Response>,
    /// The whole group, excluding the surrounding space, in UI coordinates.
    pub content_rect: Rect,
}

impl<'a> EmptyState<'a> {
    pub fn new(icon: &'a str, title: &'a str, help: &'a str) -> Self {
        Self {
            icon,
            title,
            help,
            action: None,
            viewer: false,
        }
    }

    pub fn action(mut self, label: &'a str) -> Self {
        self.action = Some((label, false));
        self
    }

    pub fn primary_action(mut self, label: &'a str) -> Self {
        self.action = Some((label, true));
        self
    }

    /// The viewer stays neutral black in every theme, so its text uses the dark ink scale.
    pub fn viewer(mut self) -> Self {
        self.viewer = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> EmptyStateResponse {
        let ink = if self.viewer {
            crate::themes::DARK.ink
        } else {
            color::ink()
        };
        let muted = if self.viewer {
            crate::themes::DARK.ink_3
        } else {
            color::ink_3()
        };
        // A parent may permit unbounded layout. The visible panel body is still finite.
        let viewport = ui
            .available_rect_before_wrap()
            .intersect(ui.clip_rect())
            .size()
            .max(vec2(0.0, 0.0));
        let width = (viewport.x - 2.0 * space::S12).clamp(1.0, 320.0);
        let layout = |text: &str, font, color| {
            let mut job = egui::text::LayoutJob::simple(text.into(), font, color, width);
            job.halign = Align::Center;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let title = layout(self.title, theme::title(), ink);
        let help = layout(self.help, theme::body(), muted);
        let action_width = self.action.map_or(0.0, |(label, _)| {
            ui.painter()
                .layout_no_wrap(label.into(), theme::body_strong(), Color32::WHITE)
                .size()
                .x
                .ceil()
                + 2.0 * space::S12
        });
        let group_width = width;
        let height = 32.0
            + space::S12
            + title.size().y
            + space::S4
            + help.size().y
            + if self.action.is_some() {
                space::S12 + crate::tokens::size::CONTROL
            } else {
                0.0
            };
        // Short bodies scroll from the top; the action remains reachable by wheel, scrollbar
        // and keyboard focus. Never turn an empty timeline/media body into a drag target.
        let source = egui::scroll_area::ScrollSource {
            drag: egui::scroll_area::DragScroll::Never,
            ..Default::default()
        };
        egui::ScrollArea::vertical()
            .id_salt("empty_state")
            .max_width(viewport.x)
            .max_height(viewport.y)
            .min_scrolled_width(0.0)
            .min_scrolled_height(0.0)
            .auto_shrink(false)
            .scroll_source(source)
            .show(ui, |ui| {
                let canvas = vec2(
                    viewport.x.max(group_width + 2.0 * space::S12),
                    viewport.y.max(height + 2.0 * space::S12),
                );
                ui.set_min_size(canvas);
                let origin = ui.min_rect().min;
                let content_rect = Rect::from_min_size(
                    origin + (canvas - vec2(group_width, height)) / 2.0,
                    vec2(group_width, height),
                );
                let center = content_rect.center().x;
                let icon = ui.painter().layout_no_wrap(
                    self.icon.into(),
                    egui::FontId::new(28.0, theme::icon_font().family),
                    muted,
                );
                ui.painter().galley(
                    pos2(center, content_rect.top() + 16.0) - icon.mesh_bounds.center().to_vec2(),
                    icon,
                    muted,
                );
                let mut y = content_rect.top() + 32.0 + space::S12;
                for galley in [title, help] {
                    let rect = Rect::from_min_size(
                        pos2(center - width / 2.0, y),
                        vec2(width, galley.size().y),
                    );
                    y += galley.size().y + space::S4;
                    // Labels expose real, wrapped text to accessibility, unlike painter-only copy.
                    ui.put(
                        rect,
                        Label::new(galley).selectable(false).sense(Sense::hover()),
                    );
                }
                let action = self.action.map(|(label, primary)| {
                    let rect = Rect::from_min_size(
                        pos2(
                            center - action_width.min(width) / 2.0,
                            y - space::S4 + space::S12,
                        ),
                        vec2(action_width.min(width), crate::tokens::size::CONTROL),
                    );
                    let button = if primary {
                        Button::primary(label)
                    } else {
                        Button::secondary(label)
                    };
                    if action_width <= width {
                        let response = ui.put(rect, button);
                        if response.gained_focus() {
                            response.scroll_to_me(Some(Align::Center));
                        }
                        response
                    } else {
                        // A long action scrolls independently: its heading and help stay centred.
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            egui::ScrollArea::horizontal()
                                .id_salt("empty_action")
                                .auto_shrink(false)
                                .min_scrolled_width(0.0)
                                .scroll_source(source)
                                .show(ui, |ui| {
                                    let response = ui.add(button);
                                    if response.gained_focus() {
                                        response.scroll_to_me(Some(Align::Center));
                                    }
                                    response
                                })
                                .inner
                        })
                        .inner
                    }
                });
                EmptyStateResponse {
                    action,
                    content_rect,
                }
            })
            .inner
    }
}
