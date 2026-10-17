//! Design tokens from `DESIGN.md` ("The Quiet Frame"). Sizes are egui points.

use egui::Color32;

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Colours. Starting values from `DESIGN.md`.
pub mod color {
    use super::hex;
    use egui::Color32;

    pub const BLUE: Color32 = hex(0x2368C9);
    pub const BLUE_HOVER: Color32 = hex(0x2C70D3);
    pub const BLUE_PRESSED: Color32 = hex(0x215DB3);
    pub const LIGHT_BLUE: Color32 = hex(0x7AACF6);

    pub const CLIP_VIOLET: Color32 = hex(0x5E589F);
    pub const CLIP_PLUM: Color32 = hex(0x8E4771);
    pub const CLIP_OCHRE: Color32 = hex(0x88602D);
    pub const CLIP_GREEN: Color32 = hex(0x30715D);
    pub const CLIPS: [Color32; 4] = [CLIP_VIOLET, CLIP_PLUM, CLIP_OCHRE, CLIP_GREEN];

    pub const RED: Color32 = hex(0xEC5B57);
    pub const AMBER: Color32 = hex(0xF2AF48);
    pub const GREEN: Color32 = hex(0x4EBE7D);

    pub const GROUND: Color32 = hex(0x0A0B0D);
    pub const PANEL: Color32 = hex(0x121416);
    pub const RAISED: Color32 = hex(0x1C1E21);
    pub const FIELD: Color32 = hex(0x0D0E11);
    pub const BORDER: Color32 = hex(0x232529);
    pub const BORDER_STRONG: Color32 = hex(0x36383C);
    pub const INK: Color32 = hex(0xEDEEF1);
    pub const INK_2: Color32 = hex(0xADB1B8);
    pub const INK_3: Color32 = hex(0x82868E);
    pub const INK_4: Color32 = hex(0x5A5E63);
    pub const VIEWER: Color32 = hex(0x040404);
    pub const WHITE: Color32 = Color32::WHITE;
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

    #[test]
    fn ink_on_every_clip_colour_reaches_4_5() {
        for clip in color::CLIPS {
            let ratio = contrast(color::INK, clip);
            assert!(ratio >= 4.5, "Ink on {clip:?} is {ratio:.2}:1");
        }
    }

    #[test]
    fn documented_text_contrasts_hold() {
        assert!(contrast(color::WHITE, color::BLUE) >= 4.5);
        assert!(contrast(color::INK_3, color::PANEL) >= 4.5);
        assert!(contrast(color::LIGHT_BLUE, color::PANEL) >= 4.5);
    }
}
