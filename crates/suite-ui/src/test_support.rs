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
