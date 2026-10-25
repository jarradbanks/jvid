//! The panel frame every dock panel uses: an 8px-radius Panel surface with a 1px
//! border (Blue when focused) and one 32px header holding a title or tabs.

use egui::emath::GuiRounding as _;
use egui::{
    CornerRadius, Rect, Sense, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::i18n::CATALOG;
use crate::theme::{self, hairline, icon};
use crate::tokens::{color, radius, size, space};
use crate::tr;
use crate::widgets::{Button, fit_text, focus_ring, hover_full, paint_fit};

/// What a panel's header shows.
pub enum Header<'a> {
    /// A single view: its title sits 12px in.
    Title(&'a str),
    /// Several views as equal-width tabs; clicking one sets `active`.
    Tabs {
        labels: &'a [&'a str],
        active: &'a mut usize,
    },
}

/// What happened in a panel this frame.
pub struct PanelResponse<R> {
    pub inner: R,
    /// The header action clicked this frame, by index.
    pub action: Option<usize>,
    pub response: egui::Response,
}

/// A dock panel filling the space it is given.
#[must_use = "call `.show(ui, ..)`"]
pub struct Panel<'a> {
    header: Header<'a>,
    actions: &'a [(&'static str, &'a str)],
    on: &'a [bool],
    focused: bool,
    wide: bool,
}

impl<'a> Panel<'a> {
    pub fn titled(title: &'a str) -> Self {
        Self::new(Header::Title(title))
    }

    pub fn tabbed(labels: &'a [&'a str], active: &'a mut usize) -> Self {
        Self::new(Header::Tabs { labels, active })
    }

    pub fn new(header: Header<'a>) -> Self {
        Self {
            header,
            actions: &[],
            on: &[],
            focused: false,
            wide: false,
        }
    }

    /// Icon buttons at the header's right: a glyph from [`icon`] and its accessible name.
    pub fn actions(mut self, actions: &'a [(&'static str, &'a str)]) -> Self {
        self.actions = actions;
        self
    }

    /// Which of the actions are toggles that are on, by index.
    pub fn on(mut self, on: &'a [bool]) -> Self {
        self.on = on;
        self
    }

    /// Tabs share the whole header instead of stopping at 160px each.
    pub fn wide(mut self, wide: bool) -> Self {
        self.wide = wide;
        self
    }

    /// Keyboard focus: swaps the border to Blue.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub fn show<R>(self, ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> PanelResponse<R> {
        let ppp = ui.pixels_per_point();
        // Round inward, so the border stays inside a clip rect on fractional pixels.
        let r = ui.available_rect_before_wrap();
        let up = |v: f32| (v * ppp).ceil() / ppp;
        let down = |v: f32| (v * ppp).floor() / ppp;
        let rect = Rect::from_min_max(
            pos2(up(r.left()), up(r.top())),
            pos2(down(r.right()), down(r.bottom())),
        );
        let response = ui.allocate_rect(rect, Sense::hover());
        let corner = CornerRadius::same(radius::PANEL);
        let painter = ui.painter().clone();
        painter.rect_filled(rect, corner, color::panel());

        let header = Rect::from_min_size(rect.min, vec2(rect.width(), size::HEADER));
        let line = 1.0 / ppp;
        painter.rect_filled(
            Rect::from_x_y_ranges(rect.x_range(), header.bottom() - line..=header.bottom()),
            0.0,
            color::border(),
        );

        let mut action = None;
        let mut right = header.right() - space::S8;
        for (i, (glyph, name)) in self.actions.iter().enumerate().rev() {
            let r = Rect::from_min_size(
                pos2(
                    right - size::CONTROL,
                    header.center().y - size::CONTROL / 2.0,
                ),
                Vec2::splat(size::CONTROL),
            );
            let on = self.on.get(i).copied().unwrap_or(false);
            let b = ui.put(r, Button::icon(glyph, name).on(on));
            if b.on_hover_text(*name).clicked() {
                action = Some(i);
            }
            right = r.left() - space::S4;
        }

        match self.header {
            Header::Title(title) => {
                let left = header.left() + space::INSET;
                let galley = fit_text(ui, title, theme::body_strong(), color::ink(), right - left);
                let pos = pos2(left, header.center().y - galley.size().y / 2.0);
                paint_fit(ui, pos.round_to_pixels(ppp), galley, color::ink());
            }
            Header::Tabs { labels, active } => {
                let right = if self.actions.is_empty() {
                    header.right() - space::S4
                } else {
                    right + space::S4
                };
                tabs(
                    ui,
                    header.left() + space::S4..=right,
                    header,
                    labels,
                    active,
                    if self.wide {
                        f32::INFINITY
                    } else {
                        size::TAB_MAX
                    },
                );
            }
        }

        let body = Rect::from_x_y_ranges(
            rect.left() + space::INSET..=rect.right() - space::INSET,
            header.bottom() + space::S8..=rect.bottom() - space::S8,
        );
        let mut child = ui.new_child(UiBuilder::new().max_rect(body));
        child.set_clip_rect(body.intersect(ui.clip_rect()));
        let inner = content(&mut child);

        let border = if self.focused {
            color::blue()
        } else {
            color::border()
        };
        painter.rect_stroke(rect, corner, hairline(ui.ctx(), border), StrokeKind::Inside);
        PanelResponse {
            inner,
            action,
            response,
        }
    }
}

/// Lays out tab pills between `x` bounds: equal widths up to `max`, 4px apart.
/// Tabs that would shrink below 64px move into a chevron menu at the end.
fn tabs(
    ui: &mut Ui,
    x: std::ops::RangeInclusive<f32>,
    header: Rect,
    labels: &[&str],
    active: &mut usize,
    max: f32,
) {
    let (left, right) = (*x.start(), *x.end());
    let avail = (right - left).max(0.0);
    let gap = space::S4;
    let fit = |n: usize, extra: f32| (avail - extra - gap * (n as f32 - 1.0)) / n as f32;
    let mut shown = labels.len();
    let mut overflow = false;
    while shown > 1
        && fit(
            shown,
            if shown < labels.len() {
                size::CONTROL + gap
            } else {
                0.0
            },
        ) < size::TAB_MIN
    {
        shown -= 1;
        overflow = true;
    }
    let extra = if overflow { size::CONTROL + gap } else { 0.0 };
    let width = fit(shown, extra).min(max).floor();
    let ppp = ui.pixels_per_point();
    let mut x = left;
    for (i, label) in labels.iter().take(shown).enumerate() {
        let pill = Rect::from_min_size(
            pos2(x, header.center().y - size::CONTROL / 2.0),
            vec2(width, size::CONTROL),
        )
        .round_to_pixels(ppp);
        x = pill.right() + gap;
        let r = ui.interact(pill, ui.id().with(("tab", i)), Sense::click());
        let on = i == *active;
        r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, on, *label));
        if r.clicked() {
            *active = i;
        }
        let fill = if on {
            color::raised()
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter()
            .rect_filled(pill, CornerRadius::same(radius::CONTROL), fill);
        let ink = if on {
            color::ink()
        } else if r.hovered() {
            color::ink_2()
        } else {
            color::ink_3()
        };
        let galley = fit_text(
            ui,
            label,
            theme::body_strong(),
            ink,
            width - 2.0 * space::S8,
        );
        let r = hover_full(r, &galley);
        let pos = pos2(
            pill.center().x - galley.size().x / 2.0,
            header.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(pos.round_to_pixels(ppp), galley, ink);
        if r.has_focus() {
            ui.painter().add(focus_ring(pill, radius::CONTROL));
        }
    }
    if overflow {
        let r = ui.put(
            Rect::from_min_size(
                pos2(x, header.center().y - size::CONTROL / 2.0),
                Vec2::splat(size::CONTROL),
            ),
            Button::icon(icon::CHEVRON_DOWN, tr!(CATALOG, "panel.more_tabs")),
        );
        egui::Popup::menu(&r).show(|ui| {
            for (i, label) in labels.iter().enumerate().skip(shown) {
                if ui.selectable_label(i == *active, *label).clicked() {
                    *active = i;
                }
            }
        });
    }
}
