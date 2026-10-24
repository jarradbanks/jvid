//! Design tokens from `DESIGN.md` ("The Quiet Frame"). Sizes are egui points.

use egui::Color32;

pub(crate) const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Colours. The themed ones are functions returning the applied theme's value
/// ([`crate::themes::apply`]); [`crate::themes::DARK`] holds the `DESIGN.md` starting values.
pub mod color {
    use super::hex;
    use egui::Color32;

    pub const BLUE_HOVER: Color32 = hex(0x2C70D3);
    pub const BLUE_PRESSED: Color32 = hex(0x215DB3);

    pub const CLIP_VIOLET: Color32 = hex(0x5E589F);
    pub const CLIP_PLUM: Color32 = hex(0x8E4771);
    pub const CLIP_OCHRE: Color32 = hex(0x88602D);
    pub const CLIP_GREEN: Color32 = hex(0x30715D);
    pub const CLIPS: [Color32; 4] = [CLIP_VIOLET, CLIP_PLUM, CLIP_OCHRE, CLIP_GREEN];

    pub const VIEWER: Color32 = hex(0x040404);
    pub const WHITE: Color32 = Color32::WHITE;

    macro_rules! themed {
        ($($name:ident),*) => {$(
            pub fn $name() -> Color32 {
                crate::themes::current().$name
            }
        )*};
    }
    themed!(
        blue,
        light_blue,
        red,
        amber,
        green,
        ground,
        panel,
        raised,
        field,
        border,
        border_strong,
        ink,
        ink_2,
        ink_3,
        ink_4
    );
}

/// Spacing scale (4px base).
pub mod space {
    pub const S4: f32 = 4.0;
    pub const S8: f32 = 8.0;
    pub const S12: f32 = 12.0;
    pub const S16: f32 = 16.0;
    pub const S24: f32 = 24.0;
    pub const S32: f32 = 32.0;
    /// Left and right padding inside a panel and its header.
    pub const INSET: f32 = S12;
    /// Between panels, and between panels and the window edge.
    pub const GUTTER: f32 = S4;
}

/// Corner radii. Every corner in the suite comes from this scale.
pub mod radius {
    pub const SMALL: u8 = 4;
    pub const CONTROL: u8 = 6;
    pub const PANEL: u8 = 8;
    pub const DIALOG: u8 = 12;
    pub const SCALE: [u8; 4] = [SMALL, CONTROL, PANEL, DIALOG];
}

/// Fixed component sizes.
pub mod size {
    pub const APP_BAR: f32 = 40.0;
    pub const HEADER: f32 = 32.0;
    pub const RULER: f32 = 24.0;
    pub const CONTROL: f32 = 24.0;
    pub const TAB_MAX: f32 = 160.0;
    pub const TAB_MIN: f32 = 64.0;
    pub const SMALL_CONTROL: f32 = 20.0;
    pub const ICON: f32 = 16.0;
    pub const TRACK_HEADER: f32 = 160.0;
    pub const TRACK: f32 = 40.0;
    pub const INSPECTOR_LABEL: f32 = 72.0;
    pub const PANEL_MIN: f32 = 240.0;
}

/// Type scale: font sizes in points.
pub mod font {
    pub const DISPLAY: f32 = 22.0;
    pub const HEADLINE: f32 = 16.0;
    pub const TITLE: f32 = 14.0;
    pub const BODY: f32 = 12.0;
    pub const SMALL: f32 = 10.0;
    pub const TIMECODE: f32 = 16.0;
}

/// WCAG 2 contrast ratio between two opaque colours.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    fn luminance(c: Color32) -> f32 {
        let lin = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::DARK;

    #[test]
    fn ink_on_every_clip_colour_reaches_4_5() {
        for clip in color::CLIPS {
            let ratio = contrast(DARK.ink, clip);
            assert!(ratio >= 4.5, "Ink on {clip:?} is {ratio:.2}:1");
        }
    }

    #[test]
    fn documented_text_contrasts_hold() {
        assert!(contrast(color::WHITE, DARK.blue) >= 4.5);
        assert!(contrast(DARK.ink_3, DARK.panel) >= 4.5);
        assert!(contrast(DARK.light_blue, DARK.panel) >= 4.5);
    }
}
