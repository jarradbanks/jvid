//! Fonts, icons and the egui `Style`/`Visuals` for the suite.

use std::sync::Arc;

use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    Style, TextStyle, Visuals,
};

use crate::tokens::{color, font, radius, size, space};

/// Lucide glyphs bundled in `assets/lucide-subset.ttf`. Draw with [`icon_font`].
pub mod icon {
    pub const CHEVRON_DOWN: &str = "\u{E071}";
    pub const CHEVRON_RIGHT: &str = "\u{E073}";
    pub const SEARCH: &str = "\u{E155}";
    pub const CHECK: &str = "\u{E070}";
    pub const X: &str = "\u{E1B2}";
    pub const PLUS: &str = "\u{E141}";
    pub const ELLIPSIS: &str = "\u{E0BA}";
    pub const CIRCLE_ALERT: &str = "\u{E07B}";
    pub const UNDO: &str = "\u{E2A1}";
    pub const REDO: &str = "\u{E2A0}";
    pub const GRID: &str = "\u{E105}";
    pub const LIST: &str = "\u{E10C}";
    pub const AUDIO: &str = "\u{E560}";
}

/// Inter Medium (500), for Body Strong, buttons, tabs and the timecode.
pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}

/// Inter SemiBold (600), for titles and headlines.
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn body() -> FontId {
    FontId::new(font::BODY, FontFamily::Proportional)
}

pub fn body_strong() -> FontId {
    FontId::new(font::BODY, medium())
}

pub fn small() -> FontId {
    FontId::new(font::SMALL, FontFamily::Proportional)
}

pub fn title() -> FontId {
    FontId::new(font::TITLE, semibold())
}

pub fn headline() -> FontId {
    FontId::new(font::HEADLINE, semibold())
}

pub fn display() -> FontId {
    FontId::new(font::DISPLAY, semibold())
}

pub fn timecode() -> FontId {
    FontId::new(font::TIMECODE, medium())
}

/// A 16px icon glyph.
pub fn icon_font() -> FontId {
    FontId::new(size::ICON, FontFamily::Name("icons".into()))
}

/// Inter (tabular figures frozen in) for every family, with the icon subset as fallback.
pub fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    let faces: [(&str, &'static [u8]); 4] = [
        ("inter", include_bytes!("../assets/Inter-Regular.ttf")),
        ("inter-medium", include_bytes!("../assets/Inter-Medium.ttf")),
        (
            "inter-semibold",
            include_bytes!("../assets/Inter-SemiBold.ttf"),
        ),
        ("lucide", include_bytes!("../assets/lucide-subset.ttf")),
    ];
    for (name, bytes) in faces {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let families = [
        (FontFamily::Proportional, "inter"),
        (FontFamily::Monospace, "inter"),
        (medium(), "inter-medium"),
        (semibold(), "inter-semibold"),
        (FontFamily::Name("icons".into()), "lucide"),
    ];
    for (family, face) in families {
        fonts
            .families
            .insert(family, vec![face.into(), "lucide".into()]);
    }
    fonts
}

/// The egui style: dark visuals, compact spacing and the suite's text styles.
pub fn style() -> Style {
    let mut style = Style {
        visuals: visuals(),
        ..Style::default()
    };
    style.text_styles = [
        (TextStyle::Small, small()),
        (TextStyle::Body, body()),
        (TextStyle::Button, body_strong()),
        (TextStyle::Heading, headline()),
        (
            TextStyle::Monospace,
            FontId::new(font::BODY, FontFamily::Monospace),
        ),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(space::S8, space::S4);
    s.button_padding = egui::vec2(space::S12, 0.0);
    s.interact_size = egui::vec2(size::CONTROL, size::CONTROL);
    s.icon_width = 14.0;
    s.icon_width_inner = 8.0;
    s.icon_spacing = space::S8;
    s.window_margin = egui::Margin::same(space::S12 as i8);
    s.menu_margin = egui::Margin::same(space::S4 as i8);
    s.combo_height = 240.0;
    s.scroll.bar_width = 4.0;
    style
}

pub fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    let control = CornerRadius::same(radius::CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = color::PANEL;
    w.noninteractive.weak_bg_fill = color::PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, color::BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, color::INK);
    w.inactive.bg_fill = color::FIELD;
    w.inactive.weak_bg_fill = color::RAISED;
    w.inactive.bg_stroke = Stroke::new(1.0, color::BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, color::INK);
    w.hovered.bg_fill = color::FIELD;
    w.hovered.weak_bg_fill = color::RAISED;
    w.hovered.bg_stroke = Stroke::new(1.0, color::BORDER_STRONG);
    w.hovered.fg_stroke = Stroke::new(1.0, color::INK);
    w.active.bg_fill = color::FIELD;
    w.active.weak_bg_fill = color::BORDER;
    w.active.bg_stroke = Stroke::new(1.0, color::BORDER_STRONG);
    w.active.fg_stroke = Stroke::new(1.0, color::INK);
    w.open = w.active;
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.corner_radius = control;
        wv.expansion = 0.0;
    }
    v.selection.bg_fill = color::BLUE.gamma_multiply(0.2);
    v.selection.stroke = Stroke::new(1.0, color::LIGHT_BLUE);
    v.hyperlink_color = color::LIGHT_BLUE;
    v.faint_bg_color = color::RAISED;
    v.extreme_bg_color = color::FIELD;
    v.code_bg_color = color::FIELD;
    v.warn_fg_color = color::AMBER;
    v.error_fg_color = color::RED;
    v.panel_fill = color::PANEL;
    v.window_fill = color::RAISED;
    v.window_stroke = Stroke::new(1.0, color::BORDER);
    v.window_corner_radius = CornerRadius::same(radius::DIALOG);
    v.menu_corner_radius = CornerRadius::same(radius::PANEL);
    let overlay = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(102),
    };
    v.window_shadow = overlay;
    v.popup_shadow = overlay;
    v.text_cursor.stroke = Stroke::new(1.0, color::LIGHT_BLUE);
    v.weak_text_color = Some(color::INK_3);
    v.button_frame = true;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v
}

/// Installs the suite's fonts and style on `ctx`. Call once at startup.
pub fn install(ctx: &Context) {
    ctx.set_fonts(fonts());
    ctx.set_global_style(style());
}

/// A border or divider: exactly one physical pixel wide at any UI scale.
pub fn hairline(ctx: &Context, color: Color32) -> Stroke {
    Stroke::new(1.0 / ctx.pixels_per_point(), color)
}
