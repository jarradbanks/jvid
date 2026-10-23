//! Base controls. Each paints its own default, hover, focus-visible, pressed and
//! disabled states (plus loading and error where they apply); `.state(..)` pins
//! one of them, which the gallery uses to show every state at once.

use egui::emath::GuiRounding as _;
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Key, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, TextEdit, Ui, Vec2, Widget, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::i18n::CATALOG;
use crate::theme::{self, hairline, icon};
use crate::tokens::{color, radius, size, space};
use crate::tr;

/// The interaction state a control is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    /// Follow the pointer and keyboard.
    #[default]
    Live,
    Default,
    Hover,
    Focus,
    Pressed,
    Disabled,
}

/// What to draw: the resolved state, and whether to add the focus ring.
#[derive(Clone, Copy)]
struct Look {
    hover: bool,
    pressed: bool,
    focus: bool,
    disabled: bool,
}

fn look(response: &Response, pinned: State) -> Look {
    match pinned {
        State::Live => Look {
            hover: response.hovered(),
            pressed: response.is_pointer_button_down_on(),
            focus: response.has_focus(),
            disabled: !response.enabled(),
        },
        s => Look {
            hover: s == State::Hover,
            pressed: s == State::Pressed,
            focus: s == State::Focus,
            disabled: s == State::Disabled,
        },
    }
}

/// Fades a colour towards its hover value over 100ms.
fn hover_fade(ui: &Ui, id: Id, hover: bool, rest: Color32, hovered: Color32) -> Color32 {
    let t = ui
        .ctx()
        .animate_bool_with_time(id.with("hover"), hover, 0.1);
    rest.lerp_to_gamma(hovered, t)
}

/// The focus-visible ring: 2px Light Blue at 60%, 1px outside the shape.
pub fn focus_ring(rect: Rect, corner: u8) -> Shape {
    Shape::rect_stroke(
        rect.expand(3.0),
        CornerRadius::same(corner + 2),
        Stroke::new(2.0, color::LIGHT_BLUE.gamma_multiply(0.6)),
        StrokeKind::Inside,
    )
}

/// A one-pixel border on a control, inside its rect.
fn border(ui: &Ui, rect: Rect, corner: u8, color: Color32) -> Shape {
    Shape::rect_stroke(
        rect,
        CornerRadius::same(corner),
        hairline(ui.ctx(), color),
        StrokeKind::Inside,
    )
}

fn text_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x
}

/// Allocates a control, disabling it first when `pinned` is `Disabled`.
fn allocate(ui: &mut Ui, size: Vec2, pinned: State, sense: Sense) -> (Rect, Response) {
    let sense = if pinned == State::Disabled {
        Sense::hover()
    } else {
        sense
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let rect = rect.round_to_pixels(ui.pixels_per_point());
    (rect, response)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonKind {
    /// One per surface: Export, or a dialog's confirm button.
    Primary,
    #[default]
    Secondary,
    /// No fill until hovered. Also the icon button.
    Ghost,
    /// Secondary styling with Red text, for confirmation dialogs.
    Destructive,
}

/// A 24px button with an optional leading icon.
#[must_use = "add it with `ui.add(..)`"]
pub struct Button<'a> {
    label: &'a str,
    kind: ButtonKind,
    icon: Option<&'static str>,
    icon_only: bool,
    loading: bool,
    on: bool,
    state: State,
    min_width: f32,
    min_height: f32,
}

impl<'a> Button<'a> {
    pub fn new(kind: ButtonKind, label: &'a str) -> Self {
        Self {
            label,
            kind,
            icon: None,
            icon_only: false,
            loading: false,
            on: false,
            state: State::Live,
            min_width: 0.0,
            min_height: 0.0,
        }
    }

    pub fn primary(label: &'a str) -> Self {
        Self::new(ButtonKind::Primary, label)
    }

    pub fn secondary(label: &'a str) -> Self {
        Self::new(ButtonKind::Secondary, label)
    }

    pub fn ghost(label: &'a str) -> Self {
        Self::new(ButtonKind::Ghost, label)
    }

    pub fn destructive(label: &'a str) -> Self {
        Self::new(ButtonKind::Destructive, label)
    }

    /// A square ghost button holding one glyph from [`icon`]; `name` is for screen readers.
    pub fn icon(glyph: &'static str, name: &'a str) -> Self {
        let mut b = Self::ghost(name).with_icon(glyph);
        b.icon_only = true;
        b
    }

    pub fn with_icon(mut self, glyph: &'static str) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// Swaps the icon for a spinner; the label and width stay.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// A ghost toggle that's on: Ink on Raised.
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }

    /// Widens the button to at least `width`, centring its content.
    pub fn width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    /// Makes the button at least `height` tall, for a large call to action.
    pub fn height(mut self, height: f32) -> Self {
        self.min_height = height;
        self
    }
}

impl Widget for Button<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = theme::body_strong();
        let has_label = !self.icon_only;
        let has_icon = self.icon.is_some() || self.loading;
        let width = if has_label {
            let icon_w = if has_icon {
                size::ICON + space::S4
            } else {
                0.0
            };
            2.0 * space::S12 + icon_w + text_width(ui, self.label, &font)
        } else {
            size::CONTROL
        };
        let pad = (self.min_width - width).max(0.0) / 2.0;
        let (rect, response) = allocate(
            ui,
            vec2(
                (width + 2.0 * pad).ceil(),
                size::CONTROL.max(self.min_height),
            ),
            self.state,
            Sense::click(),
        );
        response.widget_info(|| {
            WidgetInfo::labeled(
                WidgetType::Button,
                self.state != State::Disabled,
                self.label,
            )
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let l = look(&response, self.state);
        let (fill, ink, stroke) = if l.disabled {
            let fill = if self.kind == ButtonKind::Ghost {
                Color32::TRANSPARENT
            } else {
                color::RAISED
            };
            (fill, color::INK_4, None)
        } else {
            match self.kind {
                ButtonKind::Primary => {
                    let fill = if l.pressed {
                        color::BLUE_PRESSED
                    } else {
                        hover_fade(ui, response.id, l.hover, color::BLUE, color::BLUE_HOVER)
                    };
                    (fill, color::WHITE, None)
                }
                ButtonKind::Secondary | ButtonKind::Destructive => {
                    let ink = if self.kind == ButtonKind::Destructive {
                        color::RED
                    } else {
                        color::INK
                    };
                    let fill = if l.pressed {
                        color::BORDER
                    } else {
                        color::RAISED
                    };
                    let stroke = hover_fade(
                        ui,
                        response.id,
                        l.hover || l.pressed,
                        color::BORDER,
                        color::BORDER_STRONG,
                    );
                    (fill, ink, Some(stroke))
                }
                ButtonKind::Ghost => {
                    let lit = l.hover || self.on;
                    let fill = if l.pressed {
                        color::BORDER
                    } else {
                        hover_fade(ui, response.id, lit, Color32::TRANSPARENT, color::RAISED)
                    };
                    let ink = if lit || l.pressed {
                        color::INK
                    } else {
                        color::INK_2
                    };
                    (fill, ink, None)
                }
            }
        };
        let corner = CornerRadius::same(radius::CONTROL);
        ui.painter().rect_filled(rect, corner, fill);
        if let Some(stroke) = stroke {
            ui.painter().add(border(ui, rect, radius::CONTROL, stroke));
        }
        let mut x = if has_label {
            rect.left() + space::S12 + pad
        } else {
            rect.center().x - size::ICON / 2.0
        };
        if has_icon {
            let slot = Rect::from_min_size(
                pos2(x, rect.center().y - size::ICON / 2.0),
                Vec2::splat(size::ICON),
            );
            if self.loading {
                egui::Spinner::new()
                    .size(12.0)
                    .color(ink)
                    .paint_at(ui, Rect::from_center_size(slot.center(), Vec2::splat(12.0)));
            } else if let Some(glyph) = self.icon {
                ui.painter().text(
                    slot.center(),
                    Align2::CENTER_CENTER,
                    glyph,
                    theme::icon_font(),
                    ink,
                );
            }
            x += size::ICON + space::S4;
        }
        if has_label {
            ui.painter().text(
                pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                self.label,
                font,
                ink,
            );
        }
        if l.focus {
            ui.painter().add(focus_ring(rect, radius::CONTROL));
        }
        response
    }
}

/// A field's frame: Field fill and a Border that reacts to hover, focus and error.
fn field_frame(ui: &Ui, rect: Rect, l: Look, error: bool) -> Shape {
    let mut shapes = vec![Shape::rect_filled(
        rect,
        CornerRadius::same(radius::CONTROL),
        color::FIELD,
    )];
    let stroke = if l.disabled {
        None
    } else if error {
        Some(color::RED)
    } else if l.focus {
        Some(color::BLUE)
    } else if l.hover || l.pressed {
        Some(color::BORDER_STRONG)
    } else {
        Some(color::BORDER)
    };
    shapes.extend(stroke.map(|c| border(ui, rect, radius::CONTROL, c)));
    if l.focus && !l.disabled {
        shapes.push(focus_ring(rect, radius::CONTROL));
    }
    Shape::Vec(shapes)
}

/// Paints an optional prefix (Ink 3) at the field's left padding; returns where the value starts.
fn paint_prefix(ui: &Ui, rect: Rect, prefix: Option<&str>, glyph: Option<&str>) -> f32 {
    let mut x = rect.left() + space::S8;
    let y = rect.center().y;
    if let Some(glyph) = glyph {
        let c = pos2(x + size::ICON / 2.0, y);
        ui.painter().text(
            c,
            Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(),
            color::INK_3,
        );
        x += size::ICON + space::S4;
    }
    if let Some(prefix) = prefix {
        let r = ui.painter().text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            prefix,
            theme::body(),
            color::INK_3,
        );
        x = r.right() + space::S8;
    }
    x
}

/// Shows an error message in Red Body under a field.
fn error_line(ui: &mut Ui, error: Option<&str>) {
    if let Some(message) = error {
        ui.label(
            egui::RichText::new(message)
                .font(theme::body())
                .color(color::RED),
        );
    }
}

/// A one-line text input with an optional inline prefix, icon and error.
#[must_use = "add it with `ui.add(..)`"]
pub struct TextInput<'a> {
    text: &'a mut String,
    prefix: Option<&'a str>,
    glyph: Option<&'static str>,
    hint: &'a str,
    error: Option<&'a str>,
    width: f32,
    state: State,
}

impl<'a> TextInput<'a> {
    pub fn new(text: &'a mut String) -> Self {
        Self {
            text,
            prefix: None,
            glyph: None,
            hint: "",
            error: None,
            width: 160.0,
            state: State::Live,
        }
    }

    /// A search field: search icon prefix and an Ink 3 placeholder.
    pub fn search(text: &'a mut String) -> Self {
        let mut s = Self::new(text).hint(tr!(CATALOG, "input.search"));
        s.glyph = Some(icon::SEARCH);
        s
    }

    /// Inline prefix such as X, Y, W or H, in Ink 3.
    pub fn prefix(mut self, prefix: &'a str) -> Self {
        self.prefix = Some(prefix);
        self
    }

    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }

    /// Red border, with `message` in Red under the row.
    pub fn error(mut self, message: Option<&'a str>) -> Self {
        self.error = message;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for TextInput<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.vertical(|ui| {
            let (rect, outer) = allocate(
                ui,
                vec2(self.width, size::CONTROL),
                self.state,
                Sense::hover(),
            );
            let frame = ui.painter().add(Shape::Noop);
            let x = paint_prefix(ui, rect, self.prefix, self.glyph);
            let inner = Rect::from_x_y_ranges(x..=rect.right() - space::S8, rect.y_range());
            let disabled = self.state == State::Disabled || !ui.is_enabled();
            let ink = if disabled { color::INK_4 } else { color::INK };
            let edit = ui.add_enabled_ui(!disabled, |ui| {
                let row = ui.fonts_mut(|f| f.row_height(&theme::body()));
                let edit_rect = Rect::from_center_size(inner.center(), vec2(inner.width(), row));
                ui.put(
                    edit_rect,
                    TextEdit::singleline(self.text)
                        .frame(egui::Frame::NONE)
                        .margin(0)
                        .font(theme::body())
                        .text_color(ink)
                        .hint_text(self.hint),
                )
            });
            let mut response = edit.inner.union(outer);
            response.rect = rect;
            let mut l = look(&response, self.state);
            l.hover |= self.state == State::Live && ui.rect_contains_pointer(rect);
            ui.painter()
                .set(frame, field_frame(ui, rect, l, self.error.is_some()));
            error_line(ui, self.error);
            response
        })
        .inner
    }
}

/// A numeric field: drag sideways to scrub, double-click or press Enter to type.
#[must_use = "add it with `ui.add(..)`"]
pub struct DragField<'a> {
    value: &'a mut f64,
    prefix: Option<&'a str>,
    suffix: &'a str,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
    decimals: usize,
    width: f32,
    state: State,
    mixed: bool,
}

impl<'a> DragField<'a> {
    pub fn new(value: &'a mut f64) -> Self {
        Self {
            value,
            prefix: None,
            suffix: "",
            speed: 1.0,
            range: f64::NEG_INFINITY..=f64::INFINITY,
            decimals: 0,
            width: 80.0,
            state: State::Live,
            mixed: false,
        }
    }

    pub fn prefix(mut self, prefix: &'a str) -> Self {
        self.prefix = Some(prefix);
        self
    }

    /// Shows a dash in place of the value, for a selection whose values differ.
    pub fn mixed(mut self, mixed: bool) -> Self {
        self.mixed = mixed;
        self
    }

    pub fn suffix(mut self, suffix: &'a str) -> Self {
        self.suffix = suffix;
        self
    }

    /// Value change per point dragged.
    pub fn speed(mut self, speed: f64) -> Self {
        self.speed = speed;
        self
    }

    pub fn range(mut self, range: std::ops::RangeInclusive<f64>) -> Self {
        self.range = range;
        self
    }

    pub fn decimals(mut self, decimals: usize) -> Self {
        self.decimals = decimals;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for DragField<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, mut response) = allocate(
            ui,
            vec2(self.width, size::CONTROL),
            self.state,
            Sense::click_and_drag(),
        );
        let id = response.id;
        let frame = ui.painter().add(Shape::Noop);
        let x = paint_prefix(ui, rect, self.prefix, None);
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let mut editing: Option<String> = ui.data(|d| d.get_temp(id));
        if editing.is_none()
            && (response.double_clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))))
        {
            editing = Some(format!("{:.*}", self.decimals, self.value));
        }
        let mut focus = response.has_focus();
        if let Some(mut buffer) = editing {
            let row = ui.fonts_mut(|f| f.row_height(&theme::body()));
            let edit_rect = Rect::from_x_y_ranges(
                x..=rect.right() - space::S8,
                rect.center().y - row / 2.0..=rect.center().y + row / 2.0,
            );
            let edit = ui.put(
                edit_rect,
                TextEdit::singleline(&mut buffer)
                    .id(id.with("edit"))
                    .frame(egui::Frame::NONE)
                    .margin(0)
                    .font(theme::body())
                    .text_color(color::INK),
            );
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            focus = true;
            if edit.lost_focus() {
                if !ui.input(|i| i.key_pressed(Key::Escape))
                    && let Ok(v) = buffer
                        .trim()
                        .trim_end_matches(self.suffix)
                        .trim()
                        .parse::<f64>()
                {
                    *self.value = v.clamp(lo, hi);
                    response.mark_changed();
                }
                ui.data_mut(|d| d.remove::<String>(id));
            } else {
                ui.data_mut(|d| d.insert_temp(id, buffer));
            }
        } else {
            if response.dragged() {
                let delta = response.drag_delta().x as f64 * self.speed;
                *self.value = (*self.value + delta).clamp(lo, hi);
                response.mark_changed();
            }
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            let disabled = self.state == State::Disabled || !response.enabled();
            let ink = if disabled { color::INK_4 } else { color::INK };
            let text = match self.mixed {
                true => "\u{2014}".to_owned(),
                false => format!("{:.*}{}", self.decimals, self.value, self.suffix),
            };
            ui.painter().text(
                pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                &text,
                theme::body(),
                ink,
            );
        }
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::DragValue, true, self.prefix.unwrap_or(""))
        });
        let mut l = look(&response, self.state);
        l.focus |= self.state == State::Live && focus;
        ui.painter().set(frame, field_frame(ui, rect, l, false));
        response
    }
}

/// A field with a chevron that opens a menu of options.
#[must_use = "add it with `ui.add(..)`"]
pub struct Dropdown<'a> {
    selected: &'a mut usize,
    options: &'a [&'a str],
    width: f32,
    state: State,
}

impl<'a> Dropdown<'a> {
    pub fn new(selected: &'a mut usize, options: &'a [&'a str]) -> Self {
        Self {
            selected,
            options,
            width: 120.0,
            state: State::Live,
        }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for Dropdown<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, mut response) = allocate(
            ui,
            vec2(self.width, size::CONTROL),
            self.state,
            Sense::click(),
        );
        let current = self.options.get(*self.selected).copied().unwrap_or("");
        response.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, true, current));
        let l = look(&response, self.state);
        ui.painter().add(field_frame(ui, rect, l, false));
        let ink = if l.disabled { color::INK_4 } else { color::INK };
        let chevron = if l.disabled {
            color::INK_4
        } else {
            color::INK_2
        };
        ui.painter().text(
            pos2(rect.left() + space::S8, rect.center().y),
            Align2::LEFT_CENTER,
            current,
            theme::body(),
            ink,
        );
        ui.painter().text(
            pos2(rect.right() - space::S4 - size::ICON / 2.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon::CHEVRON_DOWN,
            theme::icon_font(),
            chevron,
        );
        egui::Popup::menu(&response).width(self.width).show(|ui| {
            for (i, option) in self.options.iter().enumerate() {
                if ui.add(MenuRow::new(option, i == *self.selected)).clicked() {
                    *self.selected = i;
                    response.mark_changed();
                }
            }
        });
        response
    }
}

/// A 24px menu row: Body Ink text with a 6px-radius Border-colour hover fill.
struct MenuRow<'a> {
    label: &'a str,
    checked: bool,
}

impl<'a> MenuRow<'a> {
    fn new(label: &'a str, checked: bool) -> Self {
        Self { label, checked }
    }
}

impl Widget for MenuRow<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(vec2(width, size::CONTROL), Sense::click());
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::SelectableLabel, true, self.checked, self.label)
        });
        if response.hovered() || response.has_focus() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius::CONTROL), color::BORDER);
        }
        ui.painter().text(
            pos2(rect.left() + space::S8, rect.center().y),
            Align2::LEFT_CENTER,
            self.label,
            theme::body(),
            color::INK,
        );
        if self.checked {
            ui.painter().text(
                pos2(rect.right() - space::S4 - size::ICON / 2.0, rect.center().y),
                Align2::CENTER_CENTER,
                icon::CHECK,
                theme::icon_font(),
                color::INK_2,
            );
        }
        response
    }
}

/// A row of mutually exclusive options in one field.
#[must_use = "add it with `ui.add(..)`"]
pub struct Segmented<'a> {
    selected: &'a mut usize,
    options: &'a [&'a str],
    state: State,
}

impl<'a> Segmented<'a> {
    pub fn new(selected: &'a mut usize, options: &'a [&'a str]) -> Self {
        Self {
            selected,
            options,
            state: State::Live,
        }
    }

    /// Pins the state of the first unselected segment.
    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for Segmented<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = theme::body_strong();
        let pad = 2.0;
        let widths: Vec<f32> = self
            .options
            .iter()
            .map(|o| (text_width(ui, o, &font) + 2.0 * space::S8).ceil())
            .collect();
        let total = widths.iter().sum::<f32>() + 2.0 * pad;
        let (rect, mut response) =
            allocate(ui, vec2(total, size::CONTROL), self.state, Sense::hover());
        let disabled = self.state == State::Disabled || !ui.is_enabled();
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::CONTROL), color::FIELD);
        if !disabled {
            ui.painter()
                .add(border(ui, rect, radius::CONTROL, color::BORDER));
        }
        let first_unselected = (0..self.options.len()).find(|i| i != self.selected);
        let mut x = rect.left() + pad;
        for (i, (option, w)) in self.options.iter().zip(&widths).enumerate() {
            let seg = Rect::from_x_y_ranges(x..=x + w, rect.top() + pad..=rect.bottom() - pad);
            x += w;
            let pinned = if Some(i) == first_unselected {
                self.state
            } else if disabled {
                State::Disabled
            } else {
                State::Live
            };
            let seg_response = ui.interact(
                seg,
                response.id.with(i),
                if disabled {
                    Sense::hover()
                } else {
                    Sense::click()
                },
            );
            let on = i == *self.selected;
            seg_response.widget_info(|| {
                WidgetInfo::selected(WidgetType::RadioButton, !disabled, on, *option)
            });
            if seg_response.clicked() {
                *self.selected = i;
                response.mark_changed();
            }
            let l = look(&seg_response, pinned);
            let l = Look {
                disabled: l.disabled || disabled,
                ..l
            };
            let fill = if on && !l.disabled {
                color::RAISED
            } else if l.pressed {
                color::BORDER
            } else {
                Color32::TRANSPARENT
            };
            ui.painter()
                .rect_filled(seg, CornerRadius::same(radius::SMALL), fill);
            let ink = match () {
                _ if l.disabled => color::INK_4,
                _ if on || l.pressed => color::INK,
                _ if l.hover => color::INK_2,
                _ => color::INK_3,
            };
            ui.painter().text(
                seg.center(),
                Align2::CENTER_CENTER,
                *option,
                font.clone(),
                ink,
            );
            if l.focus {
                ui.painter().add(focus_ring(seg, radius::SMALL));
            }
        }
        response
    }
}

/// A 14px checkbox with a label.
#[must_use = "add it with `ui.add(..)`"]
pub struct Checkbox<'a> {
    checked: &'a mut bool,
    label: &'a str,
    state: State,
}

impl<'a> Checkbox<'a> {
    pub fn new(checked: &'a mut bool, label: &'a str) -> Self {
        Self {
            checked,
            label,
            state: State::Live,
        }
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for Checkbox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let box_size = 14.0;
        let width = box_size + space::S8 + text_width(ui, self.label, &theme::body());
        let (rect, mut response) = allocate(
            ui,
            vec2(width.ceil(), size::CONTROL),
            self.state,
            Sense::click(),
        );
        if response.clicked() {
            *self.checked = !*self.checked;
            response.mark_changed();
        }
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Checkbox, true, *self.checked, self.label)
        });
        let l = look(&response, self.state);
        let ppp = ui.pixels_per_point();
        let check = Rect::from_min_size(
            pos2(rect.left(), rect.center().y - box_size / 2.0),
            Vec2::splat(box_size),
        )
        .round_to_pixels(ppp);
        let corner = CornerRadius::same(radius::SMALL);
        if *self.checked && !l.disabled {
            let fill = if l.pressed {
                color::BLUE_PRESSED
            } else if l.hover {
                color::BLUE_HOVER
            } else {
                color::BLUE
            };
            ui.painter().rect_filled(check, corner, fill);
        } else {
            let fill = if l.disabled {
                color::RAISED
            } else {
                color::FIELD
            };
            ui.painter().rect_filled(check, corner, fill);
            let stroke = if l.disabled {
                color::BORDER
            } else if l.hover || l.pressed {
                color::BORDER_STRONG
            } else {
                color::BORDER
            };
            ui.painter().add(border(ui, check, radius::SMALL, stroke));
        }
        if *self.checked {
            let ink = if l.disabled {
                color::INK_4
            } else {
                color::WHITE
            };
            ui.painter().text(
                check.center(),
                Align2::CENTER_CENTER,
                icon::CHECK,
                FontId::new(12.0, theme::icon_font().family),
                ink,
            );
        }
        let ink = if l.disabled { color::INK_4 } else { color::INK };
        ui.painter().text(
            pos2(check.right() + space::S8, rect.center().y),
            Align2::LEFT_CENTER,
            self.label,
            theme::body(),
            ink,
        );
        if l.focus {
            ui.painter().add(focus_ring(check, radius::SMALL));
        }
        response
    }
}

/// A collapsible section head on the 32px header row: chevron, then the title in
/// Body Strong, with a Border divider below. Clicking toggles `open`.
#[must_use = "add it with `ui.add(..)`"]
pub struct SectionHeader<'a> {
    title: &'a str,
    open: &'a mut bool,
    state: State,
}

impl<'a> SectionHeader<'a> {
    pub fn new(title: &'a str, open: &'a mut bool) -> Self {
        Self {
            title,
            open,
            state: State::Live,
        }
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for SectionHeader<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, mut response) = allocate(
            ui,
            vec2(ui.available_width(), size::HEADER),
            self.state,
            Sense::click(),
        );
        if response.clicked() {
            *self.open = !*self.open;
            response.mark_changed();
        }
        response
            .widget_info(|| WidgetInfo::selected(WidgetType::Button, true, *self.open, self.title));
        let l = look(&response, self.state);
        let (ink, chevron_ink) = match () {
            _ if l.disabled => (color::INK_4, color::INK_4),
            _ if l.hover || l.pressed => (color::INK, color::INK),
            _ => (color::INK, color::INK_2),
        };
        let glyph = if *self.open {
            icon::CHEVRON_DOWN
        } else {
            icon::CHEVRON_RIGHT
        };
        let y = rect.center().y;
        ui.painter().text(
            pos2(rect.left() + size::ICON / 2.0, y),
            Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(),
            chevron_ink,
        );
        ui.painter().text(
            pos2(rect.left() + size::ICON + space::S4, y),
            Align2::LEFT_CENTER,
            self.title,
            theme::body_strong(),
            ink,
        );
        let line = 1.0 / ui.pixels_per_point();
        ui.painter().rect_filled(
            Rect::from_x_y_ranges(rect.x_range(), rect.bottom() - line..=rect.bottom()),
            0.0,
            color::BORDER,
        );
        if l.focus {
            ui.painter()
                .add(focus_ring(rect.shrink2(vec2(0.0, 4.0)), radius::CONTROL));
        }
        response
    }
}

/// Formats a frame count as `HH:MM:SS:FF`.
pub fn timecode(frame: i64, fps: u32) -> String {
    let fps = i64::from(fps.max(1));
    let sign = if frame < 0 { "-" } else { "" };
    let f = frame.unsigned_abs() as i64;
    let (ff, s) = (f % fps, f / fps);
    format!(
        "{sign}{:02}:{:02}:{:02}:{:02}",
        s / 3600,
        s / 60 % 60,
        s % 60,
        ff
    )
}

/// A timecode in Timecode type (Medium 16), or Body Strong when `small`.
#[must_use = "add it with `ui.add(..)`"]
pub struct Timecode {
    frame: i64,
    fps: u32,
    small: bool,
}

impl Timecode {
    pub fn new(frame: i64, fps: u32) -> Self {
        Self {
            frame,
            fps,
            small: false,
        }
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }
}

impl Widget for Timecode {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = if self.small {
            theme::body_strong()
        } else {
            theme::timecode()
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(timecode(self.frame, self.fps))
                    .font(font)
                    .color(color::INK),
            )
            .selectable(false),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::timecode;

    #[test]
    fn timecode_formats() {
        assert_eq!(timecode(0, 30), "00:00:00:00");
        assert_eq!(timecode(30 * 3661 + 7, 30), "01:01:01:07");
        assert_eq!(timecode(-1, 25), "-00:00:00:01");
    }
}
