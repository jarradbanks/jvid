//! Base controls. Each paints its own default, hover, focus-visible, pressed and
//! disabled states (plus loading and error where they apply); `.state(..)` pins
//! one of them, which the gallery uses to show every state at once.

use egui::emath::GuiRounding as _;
use std::sync::Arc;

use egui::emath::RectAlign;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, CornerRadius, FontId, Galley, Id, Key, Rect, Response, Sense, Shape, Stroke,
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
    // No fade when the app turns animation off, as Reduce Motion does.
    let time = if ui.style().animation_time == 0.0 {
        0.0
    } else {
        0.1
    };
    let t = ui
        .ctx()
        .animate_bool_with_time(id.with("hover"), hover, time);
    rest.lerp_to_gamma(hovered, t)
}

/// Paints icon `glyph` with its ink, not its line box, centred on `center`: a Lucide glyph's
/// box runs a pixel taller than the icon, and some, such as Folder and Check, sit high in it.
pub fn paint_icon(painter: &egui::Painter, center: egui::Pos2, glyph: &str, color: Color32) {
    let galley = painter.layout_no_wrap(glyph.into(), theme::icon_font(), color);
    let pos = center - galley.mesh_bounds.center().to_vec2();
    painter.galley(pos, galley, color);
}

/// The focus-visible ring: 2px Light Blue at 60%, 1px outside the shape.
pub fn focus_ring(rect: Rect, corner: u8) -> Shape {
    Shape::rect_stroke(
        rect.expand(3.0),
        CornerRadius::same(corner + 2),
        Stroke::new(2.0, color::light_blue().gamma_multiply(0.6)),
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
    small: bool,
    ink: Option<Color32>,
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
            small: false,
            ink: None,
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

    /// A 20px icon button with 4px corners, for the tool slots of an inspector row.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
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

    /// Draws the label and icon in `ink` while enabled, such as Red for an alert.
    pub fn ink(mut self, ink: Color32) -> Self {
        self.ink = Some(ink);
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
        } else if self.small {
            size::SMALL_CONTROL
        } else {
            size::CONTROL
        };
        let side = if self.small {
            size::SMALL_CONTROL
        } else {
            size::CONTROL
        };
        let pad = (self.min_width - width).max(0.0) / 2.0;
        let (rect, response) = allocate(
            ui,
            vec2((width + 2.0 * pad).ceil(), side.max(self.min_height)),
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
                color::raised()
            };
            (fill, color::ink_4(), None)
        } else {
            match self.kind {
                ButtonKind::Primary => {
                    let fill = if l.pressed {
                        color::BLUE_PRESSED
                    } else {
                        hover_fade(ui, response.id, l.hover, color::blue(), color::BLUE_HOVER)
                    };
                    (fill, color::WHITE, None)
                }
                ButtonKind::Secondary | ButtonKind::Destructive => {
                    let ink = if self.kind == ButtonKind::Destructive {
                        color::red()
                    } else {
                        color::ink()
                    };
                    let fill = if l.pressed {
                        color::border()
                    } else {
                        color::raised()
                    };
                    let stroke = hover_fade(
                        ui,
                        response.id,
                        l.hover || l.pressed,
                        color::border(),
                        color::border_strong(),
                    );
                    (fill, ink, Some(stroke))
                }
                ButtonKind::Ghost => {
                    let lit = l.hover || self.on;
                    let fill = if l.pressed {
                        color::border()
                    } else {
                        hover_fade(ui, response.id, lit, Color32::TRANSPARENT, color::raised())
                    };
                    let ink = if lit || l.pressed {
                        color::ink()
                    } else {
                        color::ink_2()
                    };
                    (fill, ink, None)
                }
            }
        };
        let ink = self.ink.filter(|_| !l.disabled).unwrap_or(ink);
        let corner = if self.small {
            radius::SMALL
        } else {
            radius::CONTROL
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(corner), fill);
        if let Some(stroke) = stroke {
            ui.painter().add(border(ui, rect, corner, stroke));
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
                paint_icon(ui.painter(), slot.center(), glyph, ink);
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
            ui.painter().add(focus_ring(rect, corner));
        }
        response
    }
}

/// A field's frame: Field fill and a Border that reacts to hover, focus and error.
fn field_frame(ui: &Ui, rect: Rect, l: Look, error: bool) -> Shape {
    let mut shapes = vec![Shape::rect_filled(
        rect,
        CornerRadius::same(radius::CONTROL),
        color::field(),
    )];
    let stroke = if l.disabled {
        None
    } else if error {
        Some(color::red())
    } else if l.focus {
        Some(color::blue())
    } else if l.hover || l.pressed {
        Some(color::border_strong())
    } else {
        Some(color::border())
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
            color::ink_3(),
        );
        x += size::ICON + space::S4;
    }
    if let Some(prefix) = prefix {
        let r = ui.painter().text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            prefix,
            theme::body(),
            color::ink_3(),
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
                .color(color::red()),
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
            let ink = if disabled {
                color::ink_4()
            } else {
                color::ink()
            };
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
                    .text_color(color::ink()),
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
            let ink = if disabled {
                color::ink_4()
            } else {
                color::ink()
            };
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

/// One line of `text` cut to `width` with an ellipsis; check `elided` to know it was.
pub fn fit_text(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(width.max(0.0));
    ui.fonts_mut(|f| f.layout_job(job))
}

/// Paints `galley` left-aligned at `x`, centred on `y`.
fn paint_left(ui: &Ui, x: f32, y: f32, galley: Arc<Galley>, color: Color32) {
    let pos = pos2(x, y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, color);
}

/// Where a dropdown's menu opens, and its tallest scroll height: below the field unless
/// there's more room above, never past `bounds`, and `MARGIN` short of its edge.
pub fn menu_place(field: Rect, bounds: Rect, wanted: f32) -> (RectAlign, f32) {
    const MARGIN: f32 = space::S8;
    let below = bounds.bottom() - field.bottom() - MARGIN;
    let above = field.top() - bounds.top() - MARGIN;
    // The menu's frame takes its margin off both ends.
    let frame = 2.0 * space::S4;
    if wanted + frame <= below || below >= above {
        (RectAlign::BOTTOM_START, below - frame)
    } else {
        (RectAlign::TOP_START, above - frame)
    }
}

/// A field with a chevron that opens a menu of options. The menu is as wide as the field,
/// stays inside the window (or [`Dropdown::bounds`]) and scrolls past that; a name too long
/// for the field or a row ends in an ellipsis and shows in full on hover.
#[must_use = "add it with `ui.add(..)`"]
pub struct Dropdown<'a> {
    selected: &'a mut usize,
    options: &'a [&'a str],
    width: f32,
    state: State,
    bounds: Option<Rect>,
    placeholder: &'a str,
}

impl<'a> Dropdown<'a> {
    pub fn new(selected: &'a mut usize, options: &'a [&'a str]) -> Self {
        Self {
            selected,
            options,
            width: 120.0,
            state: State::Live,
            bounds: None,
            placeholder: "",
        }
    }

    /// What the field shows, in Ink 2, while `selected` is past the options, such as "Custom"
    /// for a value no preset matches. It isn't in the menu.
    pub fn placeholder(mut self, text: &'a str) -> Self {
        self.placeholder = text;
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

    /// Keeps the open menu inside `rect` as well as the window, such as a dialog's body so it
    /// never covers the dialog's buttons.
    pub fn bounds(mut self, rect: Rect) -> Self {
        self.bounds = Some(rect);
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
        let picked = self.options.get(*self.selected).copied();
        let current = picked.unwrap_or(self.placeholder);
        response.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, true, current));
        let l = look(&response, self.state);
        ui.painter().add(field_frame(ui, rect, l, false));
        let ink = match () {
            _ if l.disabled => color::ink_4(),
            _ if picked.is_none() => color::ink_2(),
            _ => color::ink(),
        };
        let chevron = if l.disabled {
            color::ink_4()
        } else {
            color::ink_2()
        };
        let chevron_x = rect.right() - space::S4 - size::ICON / 2.0;
        let room = chevron_x - size::ICON / 2.0 - space::S4 - (rect.left() + space::S8);
        let text = fit_text(ui, current, theme::body(), ink, room);
        let elided = text.elided;
        paint_left(ui, rect.left() + space::S8, rect.center().y, text, ink);
        ui.painter().text(
            pos2(chevron_x, rect.center().y),
            Align2::CENTER_CENTER,
            icon::CHEVRON_DOWN,
            theme::icon_font(),
            chevron,
        );
        let screen = ui.ctx().content_rect();
        let bounds = self.bounds.map_or(screen, |b| b.intersect(screen));
        let row = size::CONTROL + ui.spacing().item_spacing.y;
        let (align, max_height) = menu_place(rect, bounds, self.options.len() as f32 * row);
        let popup = egui::Popup::menu(&response)
            .width(rect.width())
            .align(align)
            .align_alternatives(&[])
            .show(|ui| {
                egui::ScrollArea::vertical()
                    .max_height(max_height.max(size::CONTROL))
                    .show(ui, |ui| {
                        for (i, option) in self.options.iter().enumerate() {
                            let r = ui.add(MenuRow::new(option, i == *self.selected));
                            if r.clicked() {
                                *self.selected = i;
                                response.mark_changed();
                            }
                        }
                    });
            });
        if elided && popup.is_none() {
            response = response.on_hover_text(current);
        }
        response
    }
}

/// A 24px menu row: Body Ink text with a 6px-radius Border-colour hover fill.
#[must_use = "add it with `ui.add(..)`"]
pub struct MenuRow<'a> {
    label: &'a str,
    checked: bool,
    lit: bool,
}

impl<'a> MenuRow<'a> {
    /// `checked` puts a check at the row's end.
    pub fn new(label: &'a str, checked: bool) -> Self {
        Self {
            label,
            checked,
            lit: false,
        }
    }

    /// Shows the hover fill without the pointer, such as for the row the arrow keys reached.
    pub fn lit(mut self, lit: bool) -> Self {
        self.lit = lit;
        self
    }
}

impl Widget for MenuRow<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(vec2(width, size::CONTROL), Sense::click());
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::SelectableLabel, true, self.checked, self.label)
        });
        if self.lit || response.hovered() || response.has_focus() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius::CONTROL), color::border());
        }
        let check_x = rect.right() - space::S4 - size::ICON / 2.0;
        let room = check_x - size::ICON / 2.0 - space::S4 - (rect.left() + space::S8);
        let text = fit_text(ui, self.label, theme::body(), color::ink(), room);
        let elided = text.elided;
        paint_left(
            ui,
            rect.left() + space::S8,
            rect.center().y,
            text,
            color::ink(),
        );
        if self.checked {
            ui.painter().text(
                pos2(check_x, rect.center().y),
                Align2::CENTER_CENTER,
                icon::CHECK,
                theme::icon_font(),
                color::ink_2(),
            );
        }
        if elided {
            return response.on_hover_text(self.label);
        }
        response
    }
}

/// A row of mutually exclusive options in one field.
#[must_use = "add it with `ui.add(..)`"]
pub struct Segmented<'a> {
    selected: &'a mut usize,
    options: &'a [&'a str],
    /// Each option's name when `options` are icon glyphs.
    names: Option<&'a [&'a str]>,
    state: State,
    off: &'a [Option<&'a str>],
    tips: &'a [Option<&'a str>],
}

impl<'a> Segmented<'a> {
    pub fn new(selected: &'a mut usize, options: &'a [&'a str]) -> Self {
        Self {
            selected,
            options,
            names: None,
            state: State::Live,
            off: &[],
            tips: &[],
        }
    }

    /// Icon segments: `glyphs` drawn in the icon font, each named by `names` in its tooltip.
    pub fn icons(selected: &'a mut usize, glyphs: &'a [&'a str], names: &'a [&'a str]) -> Self {
        Self {
            names: Some(names),
            ..Self::new(selected, glyphs)
        }
    }

    /// Per option: `Some(why)` disables it, with `why` on hover.
    pub fn off(mut self, off: &'a [Option<&'a str>]) -> Self {
        self.off = off;
        self
    }

    /// Per option: hover text while it's enabled.
    pub fn tips(mut self, tips: &'a [Option<&'a str>]) -> Self {
        self.tips = tips;
        self
    }

    /// Pins the state of the first unselected segment.
    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for Segmented<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = match self.names {
            Some(_) => theme::icon_font(),
            None => theme::body_strong(),
        };
        let pad = 2.0;
        let widths: Vec<f32> = self
            .options
            .iter()
            .map(|o| match self.names {
                Some(_) => size::SMALL_CONTROL,
                None => (text_width(ui, o, &font) + 2.0 * space::S8).ceil(),
            })
            .collect();
        let total = widths.iter().sum::<f32>() + 2.0 * pad;
        let (rect, mut response) =
            allocate(ui, vec2(total, size::CONTROL), self.state, Sense::hover());
        let disabled = self.state == State::Disabled || !ui.is_enabled();
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::CONTROL), color::field());
        if !disabled {
            ui.painter()
                .add(border(ui, rect, radius::CONTROL, color::border()));
        }
        let first_unselected = (0..self.options.len()).find(|i| i != self.selected);
        let mut x = rect.left() + pad;
        for (i, (option, w)) in self.options.iter().zip(&widths).enumerate() {
            let seg = Rect::from_x_y_ranges(x..=x + w, rect.top() + pad..=rect.bottom() - pad);
            x += w;
            let off = self.off.get(i).copied().flatten();
            let pinned = if Some(i) == first_unselected {
                self.state
            } else if disabled || off.is_some() {
                State::Disabled
            } else {
                State::Live
            };
            let seg_response = ui.interact(
                seg,
                response.id.with(i),
                if disabled || off.is_some() {
                    Sense::hover()
                } else {
                    Sense::click()
                },
            );
            let on = i == *self.selected;
            let name = self.names.map_or(*option, |n| n[i]);
            seg_response.widget_info(|| {
                WidgetInfo::selected(
                    WidgetType::RadioButton,
                    !disabled && off.is_none(),
                    on,
                    name,
                )
            });
            let tip = off
                .or(self.tips.get(i).copied().flatten())
                .or(self.names.map(|_| name));
            let seg_response = match tip {
                Some(tip) => seg_response.on_hover_text(tip),
                None => seg_response,
            };
            if seg_response.clicked() {
                *self.selected = i;
                response.mark_changed();
            }
            let l = look(&seg_response, pinned);
            let l = Look {
                disabled: l.disabled || disabled || off.is_some(),
                ..l
            };
            let fill = if on && !l.disabled {
                color::raised()
            } else if l.pressed {
                color::border()
            } else {
                Color32::TRANSPARENT
            };
            ui.painter()
                .rect_filled(seg, CornerRadius::same(radius::SMALL), fill);
            let ink = match () {
                _ if l.disabled => color::ink_4(),
                _ if on || l.pressed => color::ink(),
                _ if l.hover => color::ink_2(),
                _ => color::ink_3(),
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
                color::blue()
            };
            ui.painter().rect_filled(check, corner, fill);
        } else {
            let fill = if l.disabled {
                color::raised()
            } else {
                color::field()
            };
            ui.painter().rect_filled(check, corner, fill);
            let stroke = if l.disabled {
                color::border()
            } else if l.hover || l.pressed {
                color::border_strong()
            } else {
                color::border()
            };
            ui.painter().add(border(ui, check, radius::SMALL, stroke));
        }
        if *self.checked {
            let ink = if l.disabled {
                color::ink_4()
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
        let ink = if l.disabled {
            color::ink_4()
        } else {
            color::ink()
        };
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

/// Whether a property is animated, and whether it has a keyframe at the playhead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Keyed {
    #[default]
    Static,
    Animated,
    /// Animated, with a keyframe at the playhead.
    Here,
}

#[derive(Clone, Copy)]
enum KeyGlyph {
    Diamond(Keyed),
    Previous,
    Next,
}

/// A 20px keyframe control: the diamond that animates a property and shows a key at the
/// playhead, or an arrow to the previous or next keyframe.
#[must_use = "add it with `ui.add(..)`"]
pub struct KeyButton<'a> {
    glyph: KeyGlyph,
    name: &'a str,
    state: State,
}

impl<'a> KeyButton<'a> {
    /// An outline at rest, Light Blue once animated, filled when keyed at the playhead.
    pub fn diamond(keyed: Keyed, name: &'a str) -> Self {
        Self::new(KeyGlyph::Diamond(keyed), name)
    }

    pub fn previous(name: &'a str) -> Self {
        Self::new(KeyGlyph::Previous, name)
    }

    pub fn next(name: &'a str) -> Self {
        Self::new(KeyGlyph::Next, name)
    }

    fn new(glyph: KeyGlyph, name: &'a str) -> Self {
        Self {
            glyph,
            name,
            state: State::Live,
        }
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for KeyButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = allocate(
            ui,
            Vec2::splat(size::SMALL_CONTROL),
            self.state,
            Sense::click(),
        );
        let on = matches!(self.glyph, KeyGlyph::Diamond(Keyed::Here));
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::Button,
                self.state != State::Disabled,
                on,
                self.name,
            )
        });
        let l = look(&response, self.state);
        let fill = if l.pressed {
            color::border()
        } else {
            hover_fade(
                ui,
                response.id,
                l.hover,
                Color32::TRANSPARENT,
                color::raised(),
            )
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::SMALL), fill);
        let ink = match () {
            _ if l.disabled => color::ink_4(),
            _ if l.hover || l.pressed => color::ink(),
            _ => color::ink_3(),
        };
        // The timeline's diamond size, so a row's key matches its lane.
        let (c, h) = (rect.center(), 5.0);
        let shape = match self.glyph {
            KeyGlyph::Diamond(keyed) => {
                let points = vec![
                    c + vec2(0.0, -h),
                    c + vec2(h, 0.0),
                    c + vec2(0.0, h),
                    c + vec2(-h, 0.0),
                ];
                let (fill, line) = match keyed {
                    _ if l.disabled => (Color32::TRANSPARENT, color::ink_4()),
                    Keyed::Static => (Color32::TRANSPARENT, ink),
                    Keyed::Animated => (Color32::TRANSPARENT, color::light_blue()),
                    Keyed::Here => (color::light_blue(), color::light_blue()),
                };
                Shape::convex_polygon(points, fill, Stroke::new(1.0, line))
            }
            KeyGlyph::Previous | KeyGlyph::Next => {
                let dx = if matches!(self.glyph, KeyGlyph::Next) {
                    1.0
                } else {
                    -1.0
                };
                let points = vec![
                    c + vec2(-2.0 * dx, -4.0),
                    c + vec2(2.0 * dx, 0.0),
                    c + vec2(-2.0 * dx, 4.0),
                ];
                Shape::line(points, Stroke::new(1.5, ink))
            }
        };
        ui.painter().add(shape);
        if l.focus {
            ui.painter().add(focus_ring(rect, radius::SMALL));
        }
        response
    }
}

/// A slider for a ranged value: a 2px track filled from its start, or from zero when
/// `centred`, and a 6x14 thumb. Drag or click to set; arrow keys step 1% of the range.
/// Takes the width left in the row.
#[must_use = "add it with `ui.add(..)`"]
pub struct Slider<'a> {
    value: &'a mut f64,
    range: std::ops::RangeInclusive<f64>,
    step: Option<f64>,
    centred: bool,
    name: &'a str,
    state: State,
}

impl<'a> Slider<'a> {
    /// `name` is for screen readers.
    pub fn new(value: &'a mut f64, range: std::ops::RangeInclusive<f64>, name: &'a str) -> Self {
        Self {
            value,
            range,
            step: None,
            centred: false,
            name,
            state: State::Live,
        }
    }

    /// Snaps to multiples of `step` from the start, marked with ticks; arrow keys move a step.
    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    /// Fills from zero, for values that go either way from it.
    pub fn centred(mut self, centred: bool) -> Self {
        self.centred = centred;
        self
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

impl Widget for Slider<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let width = ui.available_width().max(40.0);
        let (rect, mut response) = allocate(
            ui,
            vec2(width, size::CONTROL),
            self.state,
            Sense::click_and_drag(),
        );
        let (lo, hi) = (*self.range.start(), *self.range.end());
        // The thumb's centre travels inside the track so it never overhangs the row.
        let track = rect.shrink2(vec2(3.0, 0.0)).x_range();
        let x_of =
            |v: f64| track.min + (((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32) * track.span();
        let snap = |v: f64| match self.step {
            Some(s) => (lo + ((v - lo) / s).round() * s).clamp(lo, hi),
            None => v,
        };
        if (response.dragged() || response.clicked())
            && let Some(p) = response.interact_pointer_pos()
        {
            let t = ((p.x - track.min) / track.span()).clamp(0.0, 1.0) as f64;
            let v = snap(lo + t * (hi - lo));
            if v != *self.value {
                *self.value = v;
                response.mark_changed();
            }
        }
        if response.has_focus() {
            let step = ui.input(|i| {
                i32::from(i.key_pressed(Key::ArrowRight)) - i32::from(i.key_pressed(Key::ArrowLeft))
            });
            if step != 0 {
                let by = self.step.unwrap_or((hi - lo) / 100.0);
                *self.value = snap(*self.value + f64::from(step) * by).clamp(lo, hi);
                response.mark_changed();
            }
        }
        response.widget_info(|| {
            WidgetInfo::slider(self.state != State::Disabled, *self.value, self.name)
        });
        let l = look(&response, self.state);
        let y = rect.center().y;
        let from = if self.centred { 0.0 } else { lo };
        let (a, b) = (x_of(from), x_of(*self.value));
        let painter = ui.painter();
        if self.centred {
            let line = 1.0 / ui.pixels_per_point();
            painter.rect_filled(
                Rect::from_x_y_ranges(a - line / 2.0..=a + line / 2.0, y - 5.0..=y + 5.0),
                0.0,
                color::border_strong(),
            );
        }
        let bar = |x: egui::Rangef| Rect::from_x_y_ranges(x, y - 1.0..=y + 1.0);
        painter.rect_filled(bar(track), 0.0, color::border_strong());
        if let Some(s) = self.step {
            let line = 1.0 / ui.pixels_per_point();
            for k in 0..=((hi - lo) / s).round() as usize {
                let x = x_of(lo + k as f64 * s);
                let tick =
                    Rect::from_x_y_ranges(x - line / 2.0..=x + line / 2.0, y + 3.0..=y + 6.0);
                painter.rect_filled(tick, 0.0, color::border_strong());
            }
        }
        if !l.disabled {
            painter.rect_filled(
                bar(egui::Rangef::new(a.min(b), a.max(b))),
                0.0,
                color::ink_3(),
            );
        }
        let thumb = Rect::from_center_size(pos2(b, y), vec2(6.0, 14.0))
            .round_to_pixels(ui.pixels_per_point());
        let ink = if l.disabled {
            color::ink_4()
        } else {
            let lit = l.hover || l.pressed || response.dragged();
            hover_fade(ui, response.id, lit, color::ink_2(), color::ink())
        };
        ui.painter()
            .rect_filled(thumb, CornerRadius::same(radius::SMALL), ink);
        if l.focus {
            ui.painter().add(focus_ring(thumb, radius::SMALL));
        }
        response
    }
}

/// A colour: a 24px swatch with a Border Strong edge and a checkerboard behind it only when
/// it isn't opaque, then its hex in a field, with the alpha after it when that fits, or None
/// once fully transparent. Clicking either opens egui's picker under it. Takes the width left
/// in the row; `rgba` is straight (unmultiplied) sRGB.
#[must_use = "add it with `ui.add(..)`"]
pub struct ColorField<'a> {
    rgba: &'a mut [u8; 4],
    name: &'a str,
    state: State,
}

impl<'a> ColorField<'a> {
    /// `name` is for screen readers.
    pub fn new(rgba: &'a mut [u8; 4], name: &'a str) -> Self {
        Self {
            rgba,
            name,
            state: State::Live,
        }
    }

    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }
}

/// The hex of `rgba`, and its alpha as a percentage when under 100%; `None` when fully
/// transparent.
pub fn color_text(rgba: [u8; 4]) -> Option<(String, Option<String>)> {
    let [r, g, b, a] = rgba;
    // Never 100% while not opaque.
    let alpha = (a < 255).then(|| format!("{}%", (f32::from(a) / 2.55).round().min(99.0)));
    (a > 0).then(|| (format!("{r:02X}{g:02X}{b:02X}"), alpha))
}

/// A 3x3 checkerboard filling `swatch`. Its corner cells are the light base, so the dark
/// cells never reach the rounded corners.
fn checker(ui: &Ui, swatch: Rect, corner: CornerRadius) {
    let p = ui.painter();
    p.rect_filled(swatch, corner, Color32::from_gray(0xBD));
    let cell = swatch.width() / 3.0;
    for (i, j) in [(1.0, 0.0), (0.0, 1.0), (2.0, 1.0), (1.0, 2.0)] {
        let min = swatch.min + vec2(i, j) * cell;
        p.rect_filled(
            Rect::from_min_size(min, Vec2::splat(cell)),
            0.0,
            Color32::from_gray(0x8A),
        );
    }
}

impl Widget for ColorField<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let width = ui.available_width().max(2.0 * size::CONTROL);
        let (rect, mut response) =
            allocate(ui, vec2(width, size::CONTROL), self.state, Sense::click());
        let text = color_text(*self.rgba);
        response.widget_info(|| {
            let shown = text
                .as_ref()
                .map_or(tr!(CATALOG, "color.none").to_owned(), |t| {
                    format!("{} {}", t.0, t.1.as_deref().unwrap_or_default())
                });
            WidgetInfo::labeled(
                WidgetType::ColorButton,
                self.state != State::Disabled,
                format!("{} {}", self.name, shown.trim_end()),
            )
        });
        let popup = response.id.with("picker");
        let open = egui::Popup::is_id_open(ui.ctx(), popup);
        let mut l = look(&response, self.state);
        l.focus |= open;
        let corner = CornerRadius::same(radius::CONTROL);
        let swatch = Rect::from_min_size(rect.min, Vec2::splat(size::CONTROL));
        let [r, g, b, a] = *self.rgba;
        if a < 255 {
            checker(ui, swatch, corner);
        }
        ui.painter()
            .rect_filled(swatch, corner, Color32::from_rgba_unmultiplied(r, g, b, a));
        let edge = if l.hover || l.pressed {
            color::ink_3()
        } else {
            color::border_strong()
        };
        ui.painter().add(border(ui, swatch, radius::CONTROL, edge));
        let field =
            Rect::from_x_y_ranges(swatch.right() + space::S4..=rect.right(), rect.y_range());
        ui.painter().add(field_frame(ui, field, l, false));
        let (left, right) = (field.left() + space::S8, field.right() - space::S8);
        let y = field.center().y;
        let ink = if l.disabled {
            color::ink_4()
        } else {
            color::ink()
        };
        match &text {
            None => {
                let none = tr!(CATALOG, "color.none");
                let p = pos2(left, y);
                ui.painter()
                    .text(p, Align2::LEFT_CENTER, none, theme::body(), color::ink_3());
            }
            Some((hex, alpha)) => {
                ui.painter()
                    .text(pos2(left, y), Align2::LEFT_CENTER, hex, theme::body(), ink);
                let room = right - left - text_width(ui, hex, &theme::body()) - space::S8;
                if let Some(alpha) = alpha.as_ref()
                    && text_width(ui, alpha, &theme::body()) <= room
                {
                    let p = pos2(right, y);
                    ui.painter().text(
                        p,
                        Align2::RIGHT_CENTER,
                        alpha,
                        theme::body(),
                        color::ink_3(),
                    );
                }
            }
        }
        let mut changed = false;
        egui::Popup::menu(&response)
            .id(popup)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                // The picker's own hue, so it survives a pass through grey or black.
                let key = popup.with("hsva");
                let rgba = *self.rgba;
                let mut hsva = ui
                    .data(|d| d.get_temp::<([u8; 4], egui::ecolor::Hsva)>(key))
                    .filter(|(c, _)| *c == rgba)
                    .map_or_else(
                        || egui::ecolor::Hsva::from_srgba_unmultiplied(rgba),
                        |(_, h)| h,
                    );
                let alpha = egui::color_picker::Alpha::OnlyBlend;
                if egui::color_picker::color_picker_hsva_2d(ui, &mut hsva, alpha) {
                    *self.rgba = hsva.to_srgba_unmultiplied();
                    changed = true;
                }
                ui.data_mut(|d| d.insert_temp(key, (*self.rgba, hsva)));
            });
        if changed {
            response.mark_changed();
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
            _ if l.disabled => (color::ink_4(), color::ink_4()),
            _ if l.hover || l.pressed => (color::ink(), color::ink()),
            _ => (color::ink(), color::ink_2()),
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
            color::border(),
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
                    .color(color::ink()),
            )
            .selectable(false),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{RectAlign, menu_place, timecode};
    use egui::{Rect, pos2};

    #[test]
    fn menus_open_toward_the_room_and_stop_at_the_bounds() {
        let bounds = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 500.0));
        let field = |y: f32| Rect::from_min_max(pos2(10.0, y), pos2(200.0, y + 28.0));
        // A short list fits below.
        assert_eq!(
            menu_place(field(100.0), bounds, 100.0).0,
            RectAlign::BOTTOM_START
        );
        // A long one near the bottom flips up and is capped to the room there.
        let (align, h) = menu_place(field(400.0), bounds, 1000.0);
        assert_eq!(align, RectAlign::TOP_START);
        assert!(h < 400.0 - 8.0, "{h}");
        // With more room below it stays below, capped.
        let (align, h) = menu_place(field(50.0), bounds, 1000.0);
        assert_eq!(align, RectAlign::BOTTOM_START);
        assert!(50.0 + 28.0 + h < 500.0, "{h}");
    }

    #[test]
    fn timecode_formats() {
        assert_eq!(timecode(0, 30), "00:00:00:00");
        assert_eq!(timecode(30 * 3661 + 7, 30), "01:01:01:07");
        assert_eq!(timecode(-1, 25), "-00:00:00:01");
    }
}
