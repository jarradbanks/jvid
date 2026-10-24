//! Fonts, icons and the egui `Style`/`Visuals` for the suite.

use std::sync::{Arc, Mutex};

use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    Style, TextStyle, Visuals,
};

use crate::i18n;
use crate::themes::{self, Palette};
use crate::tokens::{font, radius, size, space};

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
    pub const PLAY: &str = "\u{E140}";
    pub const PAUSE: &str = "\u{E132}";
    pub const SKIP_BACK: &str = "\u{E163}";
    pub const SKIP_FORWARD: &str = "\u{E164}";
    pub const REWIND: &str = "\u{E14B}";
    pub const FAST_FORWARD: &str = "\u{E0C1}";
    pub const LOCK: &str = "\u{E10F}";
    pub const EYE: &str = "\u{E0BE}";
    pub const EYE_OFF: &str = "\u{E0BF}";
    pub const POINTER: &str = "\u{E1C3}";
    pub const SCISSORS: &str = "\u{E152}";
    pub const MAGNET: &str = "\u{E2B5}";
    pub const LINK: &str = "\u{E108}";
    pub const ZOOM_IN: &str = "\u{E1B6}";
    pub const ZOOM_OUT: &str = "\u{E1B7}";
    pub const SCAN: &str = "\u{E257}";
    pub const SPLIT: &str = "\u{E4EE}";
    pub const SLIDERS: &str = "\u{E29A}";
    pub const MIC: &str = "\u{E11C}";
    pub const FOLDER: &str = "\u{E0DC}";
    pub const ALIGN_LEFT: &str = "\u{E041}";
    pub const ALIGN_CENTER: &str = "\u{E03C}";
    pub const ALIGN_RIGHT: &str = "\u{E042}";
    pub const CLOCK: &str = "\u{E08B}";
    pub const BAN: &str = "\u{E055}";
    pub const CIRCLE_CHECK: &str = "\u{E226}";
    pub const LOADER: &str = "\u{E10E}";
    pub const PANEL_BOTTOM: &str = "\u{E431}";
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

/// Each CJK language's Noto Sans CJK Regular, subset to its catalogs by
/// `assets/subset-cjk.py` and used for every weight. Loaded only while its language is active,
/// because each language draws Han characters its own way.
pub const CJK_FACES: [(&str, &[u8]); 3] = [
    ("ja", include_bytes!("../assets/NotoSansCJK-ja.otf")),
    (
        "zh-Hans",
        include_bytes!("../assets/NotoSansCJK-zh-Hans.otf"),
    ),
    ("ko", include_bytes!("../assets/NotoSansCJK-ko.otf")),
];

/// The font data name of `lang`'s CJK face.
pub fn cjk_face(lang: &str) -> String {
    format!("noto-cjk-{lang}")
}

/// Inter (tabular figures frozen in) for every family, then `lang`'s CJK face if it has one,
/// the CJK language names (the Settings picker shows them in every language) and the icon
/// subset as fallbacks.
pub fn fonts(lang: &str) -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    let cjk = CJK_FACES.iter().find(|(l, _)| *l == lang);
    let faces: [(&str, &'static [u8]); 5] = [
        ("inter", include_bytes!("../assets/Inter-Regular.ttf")),
        ("inter-medium", include_bytes!("../assets/Inter-Medium.ttf")),
        (
            "inter-semibold",
            include_bytes!("../assets/Inter-SemiBold.ttf"),
        ),
        ("lucide", include_bytes!("../assets/lucide-subset.ttf")),
        (
            "cjk-names",
            include_bytes!("../assets/NotoSansCJK-names.otf"),
        ),
    ];
    for (name, bytes) in faces {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    if let Some((l, bytes)) = cjk {
        fonts
            .font_data
            .insert(cjk_face(l), Arc::new(FontData::from_static(bytes)));
    }
    let fallbacks: Vec<String> = cjk
        .map(|(l, _)| cjk_face(l))
        .into_iter()
        .chain(["cjk-names".into(), "lucide".into()])
        .collect();
    let families = [
        (FontFamily::Proportional, "inter"),
        (FontFamily::Monospace, "inter"),
        (medium(), "inter-medium"),
        (semibold(), "inter-semibold"),
        (FontFamily::Name("icons".into()), "lucide"),
    ];
    for (family, face) in families {
        let chain = std::iter::once(face.to_owned()).chain(fallbacks.iter().cloned());
        fonts.families.insert(family, chain.collect());
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
    visuals_with(&themes::DARK)
}

/// The suite's visuals in `p`'s colours.
pub fn visuals_with(p: &Palette) -> Visuals {
    let color = p;
    // A light panel needs egui's light defaults for anything not set below.
    let mut v = if crate::tokens::contrast(p.panel, egui::Color32::BLACK) > 10.0 {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    let control = CornerRadius::same(radius::CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = color.panel;
    w.noninteractive.weak_bg_fill = color.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, color.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, color.ink);
    w.inactive.bg_fill = color.field;
    w.inactive.weak_bg_fill = color.raised;
    w.inactive.bg_stroke = Stroke::new(1.0, color.border);
    w.inactive.fg_stroke = Stroke::new(1.0, color.ink);
    w.hovered.bg_fill = color.field;
    w.hovered.weak_bg_fill = color.raised;
    w.hovered.bg_stroke = Stroke::new(1.0, color.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, color.ink);
    w.active.bg_fill = color.field;
    w.active.weak_bg_fill = color.border;
    w.active.bg_stroke = Stroke::new(1.0, color.border_strong);
    w.active.fg_stroke = Stroke::new(1.0, color.ink);
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
    v.selection.bg_fill = color.blue.gamma_multiply(0.2);
    v.selection.stroke = Stroke::new(1.0, color.light_blue);
    v.hyperlink_color = color.light_blue;
    v.faint_bg_color = color.raised;
    v.extreme_bg_color = color.field;
    v.code_bg_color = color.field;
    v.warn_fg_color = color.amber;
    v.error_fg_color = color.red;
    v.panel_fill = color.panel;
    v.window_fill = color.raised;
    v.window_stroke = Stroke::new(1.0, color.border);
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
    v.text_cursor.stroke = Stroke::new(1.0, color.light_blue);
    v.weak_text_color = Some(color.ink_3);
    v.button_frame = true;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v
}

/// Installs the suite's fonts and style on `ctx`. Call once at startup.
pub fn install(ctx: &Context) {
    ctx.set_fonts(fonts(i18n::language()));
    ctx.set_global_style(style());
    ctx.on_begin_pass("hairlines", Arc::new(|ui| sync_hairlines(ui.ctx())));
    // A language switch swaps the CJK face; the new fonts draw from the next pass.
    let shown = Mutex::new(i18n::language());
    ctx.on_begin_pass(
        "language fonts",
        Arc::new(move |ui| {
            let mut shown = shown.lock().unwrap_or_else(|e| e.into_inner());
            if *shown != i18n::language() {
                *shown = i18n::language();
                ui.ctx().set_fonts(fonts(*shown));
                ui.ctx().request_repaint();
            }
        }),
    );
}

/// egui draws its own borders (popups, menus, separators) at 1pt, which is 1.5 physical
/// pixels at 150%; this keeps them one physical pixel at the current scale.
fn sync_hairlines(ctx: &Context) {
    let width = 1.0 / ctx.pixels_per_point();
    if ctx.global_style().visuals.window_stroke.width == width {
        return;
    }
    ctx.global_style_mut(|s| {
        let v = &mut s.visuals;
        v.window_stroke.width = width;
        let w = &mut v.widgets;
        for wv in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            wv.bg_stroke.width = width;
        }
    });
}

/// A border or divider: exactly one physical pixel wide at any UI scale.
pub fn hairline(ctx: &Context, color: Color32) -> Stroke {
    Stroke::new(1.0 / ctx.pixels_per_point(), color)
}
