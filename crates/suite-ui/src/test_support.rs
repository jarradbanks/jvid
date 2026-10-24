//! GPU test setup shared by rendering tests here and in the apps (feature `test-support`).

use std::path::Path;
use std::sync::OnceLock;

const LAVAPIPE: &str = "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json";

/// Call first in every rendering test; `false` means skip it.
///
/// Points wgpu at Mesa lavapipe (software Vulkan) when it is installed and no driver was chosen,
/// because the snapshots are rendered with it and WSL2's default driver segfaults. Without
/// lavapipe the tests skip, unless `JVID_GPU_TESTS=required`, which runs them on whatever
/// driver there is so CI can't pass by skipping.
pub fn gpu() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| {
        let required = std::env::var("JVID_GPU_TESTS").is_ok_and(|v| v == "required");
        if std::env::var_os("VK_ICD_FILENAMES").is_none() && Path::new(LAVAPIPE).exists() {
            // SAFETY: runs once, before any wgpu init; other tests wait on this OnceLock.
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var("VK_ICD_FILENAMES", LAVAPIPE);
                std::env::set_var("WGPU_BACKEND", "vulkan");
            }
            return true;
        }
        if required || std::env::var_os("VK_ICD_FILENAMES").is_some() {
            return true;
        }
        eprintln!(
            "skipping GPU tests: Mesa lavapipe not found at {LAVAPIPE}. Install mesa-vulkan-drivers, \
             or set VK_ICD_FILENAMES, or JVID_GPU_TESTS=required to use the default driver."
        );
        false
    })
}

/// Installs the suite theme on a harness's first frame, which then draws nothing:
/// egui 0.36's kittest has no context setup hook and fonts apply from the next frame.
/// Returns true on that first frame.
pub fn install_theme(ui: &egui::Ui, fresh: &mut bool) -> bool {
    if std::mem::take(fresh) {
        crate::theme::install(ui.ctx());
        ui.ctx().request_repaint();
        return true;
    }
    false
}

/// Each character of `catalog`'s text, in every language and drawn with that language's fonts,
/// that the theme can't draw (egui shows its replacement glyph instead), as `lang key: 'c'`.
pub fn missing_glyphs(catalog: &crate::i18n::Catalog) -> Vec<String> {
    use crate::theme;
    let mut out = Vec::new();
    for lang in crate::i18n::LANGUAGES {
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::fonts(lang));
        let mut found = Vec::new();
        // The fonts load on the first pass.
        for _ in 0..2 {
            let mut pass = ctx.run_ui(Default::default(), |ui| {
                // egui's `has_glyph` says no to everything when the main face also holds the
                // replacement glyph, so compare against what a code point no font has draws as.
                let uv = |c: char, font: &egui::FontId| {
                    let g =
                        ui.painter()
                            .layout_no_wrap(c.into(), font.clone(), egui::Color32::WHITE);
                    g.rows[0].glyphs[0].uv_rect
                };
                found.clear();
                for font in [theme::body(), theme::body_strong(), theme::title()] {
                    let none = uv('\u{10FFFD}', &font);
                    for key in catalog.keys(lang) {
                        let text = catalog.get_in(lang, key).unwrap_or_default();
                        for c in text.chars().filter(|c| !c.is_whitespace() && *c != '?') {
                            if uv(c, &font) == none {
                                found.push(format!("{lang} {key}: {c:?}"));
                            }
                        }
                    }
                }
            });
            pass.textures_delta.clear();
        }
        out.extend(found);
    }
    out.sort();
    out.dedup();
    out
}

/// Test catalog sources: English and Spanish as given, every other language empty.
pub const fn en_es(
    en: &'static str,
    es: &'static str,
) -> [&'static str; crate::i18n::LANGUAGES.len()] {
    let mut s = [""; crate::i18n::LANGUAGES.len()];
    s[0] = en;
    s[1] = es;
    s
}
